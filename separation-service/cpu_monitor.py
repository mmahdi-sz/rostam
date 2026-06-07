"""
CPU usage monitor — reads /proc/stat, keeps 5-minute sliding window,
computes per-core idle percentages and decides how many cores to lend.

Decision logic:
  - If 2-min avg > 80% OR 5-min avg > 50%  → 0 cores (queue)
  - current idle > 50%  → 4 cores
  - current idle > 25%  → 2 cores
  - current idle > 6%   → 1 core
  - else                → 0 cores (queue)
"""

import time
import asyncio
from collections import deque
from dataclasses import dataclass, field
from typing import List, Deque


@dataclass
class CoreSample:
    timestamp: float
    idle_pct: float          # 0-100


@dataclass
class CpuMonitor:
    num_cores: int = 0
    _history: List[Deque[CoreSample]] = field(default_factory=list)
    _prev_stats: List[dict] = field(default_factory=list)
    _lock: asyncio.Lock = field(default_factory=asyncio.Lock)
    _running: bool = False

    WINDOW_SECS = 300          # 5 minutes
    SAMPLE_INTERVAL = 2        # seconds between reads
    TWO_MIN = 120
    FIVE_MIN = 300

    def _read_proc_stat(self):
        cores = []
        with open("/proc/stat") as f:
            for line in f:
                if not line.startswith("cpu") or line.startswith("cpu "):
                    continue
                parts = line.split()
                name = parts[0]
                vals = list(map(int, parts[1:]))
                # user nice system idle iowait irq softirq steal guest guest_nice
                idle = vals[3] + (vals[4] if len(vals) > 4 else 0)
                total = sum(vals)
                cores.append({"name": name, "idle": idle, "total": total})
        return cores

    async def start(self):
        self.num_cores = 0
        initial = self._read_proc_stat()
        self.num_cores = len(initial)
        self._prev_stats = initial
        self._history = [deque() for _ in range(self.num_cores)]
        self._running = True
        asyncio.create_task(self._loop())

    async def _loop(self):
        while self._running:
            await asyncio.sleep(self.SAMPLE_INTERVAL)
            await self._sample()

    async def _sample(self):
        now = time.monotonic()
        curr = self._read_proc_stat()
        async with self._lock:
            cutoff = now - self.WINDOW_SECS
            for i in range(min(len(curr), self.num_cores)):
                prev = self._prev_stats[i]
                d_idle = curr[i]["idle"] - prev["idle"]
                d_total = curr[i]["total"] - prev["total"]
                idle_pct = (d_idle / d_total * 100) if d_total > 0 else 100.0
                self._history[i].append(CoreSample(now, idle_pct))
                # purge old samples
                while self._history[i] and self._history[i][0].timestamp < cutoff:
                    self._history[i].popleft()
            self._prev_stats = curr

    def _avg_idle(self, window_secs: float, now: float) -> float:
        """Average idle % across all cores for the given window."""
        cutoff = now - window_secs
        total_idle = 0.0
        count = 0
        for dq in self._history:
            for s in dq:
                if s.timestamp >= cutoff:
                    total_idle += s.idle_pct
                    count += 1
        return (total_idle / count) if count > 0 else 100.0

    def _current_idle(self) -> float:
        """Most recent idle sample averaged across all cores."""
        total = 0.0
        count = 0
        for dq in self._history:
            if dq:
                total += dq[-1].idle_pct
                count += 1
        return (total / count) if count > 0 else 100.0

    async def available_cores(self) -> int:
        """Return how many cores can be lent right now (0 = queue)."""
        async with self._lock:
            now = time.monotonic()
            avg_2min = 100 - self._avg_idle(self.TWO_MIN, now)   # → usage %
            avg_5min = 100 - self._avg_idle(self.FIVE_MIN, now)
            current_idle = self._current_idle()

        if avg_2min > 80 or avg_5min > 50:
            return 0

        if current_idle > 50:
            return 4
        if current_idle > 25:
            return 2
        if current_idle > 6:
            return 1
        return 0

    async def pick_cores(self, count: int) -> List[int]:
        """
        Pick `count` specific core indices that are most idle right now.
        Returns list of core indices (0-based).
        """
        async with self._lock:
            idleness = []
            for i, dq in enumerate(self._history):
                idle = dq[-1].idle_pct if dq else 100.0
                idleness.append((idle, i))
        idleness.sort(reverse=True)
        return [idx for _, idx in idleness[:count]]

    def stop(self):
        self._running = False


# Module-level singleton
_monitor = CpuMonitor()


async def start_monitor():
    await _monitor.start()


async def available_cores() -> int:
    return await _monitor.available_cores()


async def pick_cores(count: int) -> List[int]:
    return await _monitor.pick_cores(count)
