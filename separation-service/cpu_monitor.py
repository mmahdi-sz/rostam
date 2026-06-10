"""
CPU usage monitor — reads /proc/stat, keeps 5-minute sliding window,
computes per-core idle percentages and decides how many cores to lend.

Decision logic (based on current 30s avg usage %):
  < 50%  → 4 cores
  < 75%  → 2 cores
  < 94%  → 1 core
  >= 94% → 0 cores (queue)

Overload detection (written to Redis by cpu_broker):
  3-min avg usage > 50% AND 1-min avg usage > 80%
  → cpu:overloaded key set in Redis with TTL=300s
  → available_cores() returns 0 until key expires
"""

import time
import asyncio
from collections import deque
from dataclasses import dataclass, field
from typing import List, Deque, Optional, Callable, Awaitable


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
    # Injected by cpu_broker after startup so we can write to Redis.
    _overload_callback: Optional[Callable[[], Awaitable[None]]] = None

    WINDOW_SECS = 300          # 5 minutes
    SAMPLE_INTERVAL = 2        # seconds between reads
    ONE_MIN = 60
    THREE_MIN = 180

    def _read_proc_stat(self):
        cores = []
        with open("/proc/stat") as f:
            for line in f:
                if not line.startswith("cpu") or line.startswith("cpu "):
                    continue
                parts = line.split()
                vals = list(map(int, parts[1:]))
                # user nice system idle iowait irq softirq steal guest guest_nice
                idle = vals[3] + (vals[4] if len(vals) > 4 else 0)
                total = sum(vals)
                cores.append({"name": parts[0], "idle": idle, "total": total})
        return cores

    async def start(self):
        self.num_cores = 0
        initial = self._read_proc_stat()
        self.num_cores = len(initial)
        self._prev_stats = initial
        self._history = [deque() for _ in range(self.num_cores)]
        self._running = True
        asyncio.create_task(self._loop())

    def set_overload_callback(self, cb: Callable[[], Awaitable[None]]):
        self._overload_callback = cb

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
                while self._history[i] and self._history[i][0].timestamp < cutoff:
                    self._history[i].popleft()
            self._prev_stats = curr

        # Check overload after every sample (outside lock).
        if self._overload_callback is not None:
            now2 = time.monotonic()
            usage_1min = 100 - self._avg_idle(self.ONE_MIN, now2)
            usage_3min = 100 - self._avg_idle(self.THREE_MIN, now2)
            if usage_3min > 50 and usage_1min > 80:
                asyncio.create_task(self._overload_callback())

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
        """Most recent sample averaged across all cores."""
        total = 0.0
        count = 0
        for dq in self._history:
            if dq:
                total += dq[-1].idle_pct
                count += 1
        return (total / count) if count > 0 else 100.0

    async def available_cores(self) -> int:
        """
        Return how many cores to lend right now.
        0 means queue. Does NOT check cpu:overloaded — caller does that.
        """
        async with self._lock:
            current_usage = 100 - self._current_idle()

        if current_usage < 50:
            return 4
        if current_usage < 75:
            return 2
        if current_usage < 94:
            return 1
        return 0

    async def pick_cores(self, count: int) -> List[int]:
        """Pick `count` most-idle core indices (0-based)."""
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


def set_overload_callback(cb: Callable[[], Awaitable[None]]):
    _monitor.set_overload_callback(cb)
