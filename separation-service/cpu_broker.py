"""
CPU Broker — manages core reservations via Redis.

Redis keys used:
  cpu:reserved          Hash  {core_idx → "user_id:expire_ts"}
  cpu:queue             Sorted Set  {ticket_json → score}
                        score = priority * 1e12 + timestamp_ms
                        priority: 1=VIP, 2=normal
  cpu:notify            Pub/Sub channel — broker publishes "" on each release

Ticket JSON: {"ticket_id": str, "user_id": int, "is_vip": bool, "ts": float}

acquire(user_id, is_vip) → List[int] of core indices, or raises QueueFull
release(cores)           → frees cores, notifies queue
"""

import asyncio
import json
import os
import time
import uuid
from typing import List, Optional

import redis.asyncio as aioredis

from cpu_monitor import available_cores, pick_cores

REDIS_URL = os.getenv("REDIS_URL", "redis://127.0.0.1:6379")
RESERVE_TTL = 900           # 15 min: max reservation lifetime (seconds)
NOTIFY_CHANNEL = "cpu:notify"
RESERVED_KEY = "cpu:reserved"
QUEUE_KEY = "cpu:queue"

_redis: Optional[aioredis.Redis] = None


async def get_redis() -> aioredis.Redis:
    global _redis
    if _redis is None:
        _redis = aioredis.from_url(REDIS_URL, decode_responses=True)
    return _redis


class QueuedRequest:
    """Awaitable handle returned when cores aren't available."""
    def __init__(self, ticket_id: str, event: asyncio.Event):
        self.ticket_id = ticket_id
        self.event = event


# In-process waiters: ticket_id → asyncio.Event
_waiters: dict[str, asyncio.Event] = {}
_listener_task: Optional[asyncio.Task] = None


async def start_broker():
    """Start the Redis pub/sub listener. Call once at service startup."""
    global _listener_task
    _listener_task = asyncio.create_task(_listen_releases())


async def _listen_releases():
    r = await get_redis()
    pubsub = r.pubsub()
    await pubsub.subscribe(NOTIFY_CHANNEL)
    async for message in pubsub.listen():
        if message["type"] != "message":
            continue
        # Wake all local waiters — they'll re-check availability themselves.
        for event in list(_waiters.values()):
            event.set()


async def _try_acquire_now(user_id: int) -> Optional[List[int]]:
    """Attempt to grab cores right now. Returns core list or None."""
    r = await get_redis()

    # Purge stale reservations first.
    now_ts = time.time()
    all_reserved = await r.hgetall(RESERVED_KEY)
    for core_str, val in all_reserved.items():
        _, expire_str = val.split(":", 1)
        if float(expire_str) < now_ts:
            await r.hdel(RESERVED_KEY, core_str)

    count = await available_cores()
    if count == 0:
        return None

    reserved_cores = set(int(k) for k in (await r.hgetall(RESERVED_KEY)).keys())
    candidates = await pick_cores(count * 2)   # get extra candidates in case some reserved
    free = [c for c in candidates if c not in reserved_cores][:count]

    if not free:
        return None

    expire_ts = now_ts + RESERVE_TTL
    pipe = r.pipeline()
    for c in free:
        pipe.hset(RESERVED_KEY, str(c), f"{user_id}:{expire_ts}")
    await pipe.execute()
    return free


async def acquire(user_id: int, is_vip: bool = False) -> List[int]:
    """
    Get core indices for this user.
    Blocks until cores are available (no timeout — caller manages timeout).
    Raises RuntimeError if enqueue fails.
    """
    # Fast path — try immediately.
    cores = await _try_acquire_now(user_id)
    if cores is not None:
        return cores

    # Slow path — enqueue and wait for a release notification.
    ticket_id = str(uuid.uuid4())
    event = asyncio.Event()
    _waiters[ticket_id] = event

    r = await get_redis()
    priority = 1 if is_vip else 2
    score = priority * 1_000_000_000_000 + int(time.time() * 1000)
    ticket = json.dumps({"ticket_id": ticket_id, "user_id": user_id,
                         "is_vip": is_vip, "ts": time.time()})
    await r.zadd(QUEUE_KEY, {ticket: score})

    try:
        while True:
            event.clear()
            cores = await _try_acquire_now(user_id)
            if cores is not None:
                await r.zrem(QUEUE_KEY, ticket)
                return cores
            await event.wait()
    finally:
        _waiters.pop(ticket_id, None)


async def cancel_ticket(ticket_id: str):
    """Remove a queued ticket (user cancelled)."""
    _waiters.pop(ticket_id, None)
    r = await get_redis()
    # Find and remove the matching ticket in the sorted set.
    members = await r.zrange(QUEUE_KEY, 0, -1)
    for m in members:
        try:
            data = json.loads(m)
            if data.get("ticket_id") == ticket_id:
                await r.zrem(QUEUE_KEY, m)
                break
        except Exception:
            pass


async def release(cores: List[int]):
    """Free reserved cores and notify waiting processes."""
    r = await get_redis()
    pipe = r.pipeline()
    for c in cores:
        pipe.hdel(RESERVED_KEY, str(c))
    await pipe.execute()
    await r.publish(NOTIFY_CHANNEL, "release")


async def queue_position(ticket_id: str) -> int:
    """Return 1-based position in queue, or 0 if not found."""
    r = await get_redis()
    members = await r.zrange(QUEUE_KEY, 0, -1)
    for i, m in enumerate(members):
        try:
            data = json.loads(m)
            if data.get("ticket_id") == ticket_id:
                return i + 1
        except Exception:
            pass
    return 0
