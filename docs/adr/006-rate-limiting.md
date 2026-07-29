# ADR 006: Multi-Tier Quota & Rate Limiting

## Context
Preventing bot abuse, traffic saturation, and cookie ban cascades requires multi-tier rate limiting for features and YouTube cookies.

## Decision
- Enforce daily/monthly traffic quotas per user rank (`Dalavar`, `Sepahbod`, `Esfandyar`, `Sohrab`, `Rostam`).
- Implement Redis-backed cookie cooldown queues for YouTube requests (30-minute lock window on 429 rate limit).
- Reject SSRF attempts and invalid URLs before resource allocation via central `src/validation.rs`.

## Consequences
- Protects downstream services and Firefox cookie profiles from IP bans.
- Fair usage enforcement across user ranks with paywall fallback triggers.
