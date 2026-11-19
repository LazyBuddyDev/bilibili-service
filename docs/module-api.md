# Bilibili service API

Stdin/stdout JSON-lines protocol v1, 64 KiB input limit. Envelope: `{ "version":1, "id":"request", "method":"health", "args":{} }`. Successful replies contain `ok:true,data`; failures contain only a bounded stable `error.code`. The service writes no logs. Private cookies and upstream signed addresses never appear in replies.

## Methods

- `health`: version/capabilities and current media request/byte counters.
- `search {query,page}`: asynchronous job; returns title/author/duration and BV IDs. Video results only; paid course cards are ignored. HTML highlights become plain text. At most 50 results/page and 1000 pages.
- `task.status {job}`: pending/complete/failed/cancelled, with result only for the current job. `task.cancel {job}` cancels; cancelled workers still occupy the single network-work slot until finished. Results are committed only when current and uncancelled.
- `account.get`, `account.restore`, `account.login`, `account.poll {attempt}`, `account.cancel {attempt}`, `account.logout`. Restore/login/poll are asynchronous. QR replies contain an SVG data URI and attempt ID, never a raw QR key. Login is confirmed only after official nav identity verification and successful Windows Credential Manager save. Cancel clears the attempt; only logout deletes this app's scoped saved credential. Expired credentials are preserved until explicit logout or successful replacement.
- `recommend {fresh}`: official randomized homepage video recommendations, manually refreshed; omit commercial/live/non-video cards.
- `following {offset}`: official authenticated followed-UP video feed, explicit pagination; preserve returned cursor, no timer/prefetch.
- `playback.open {bvid,cid?,origin}`: asynchronous; selected CID is revalidated against the video pages; missing CID selects the first P. Complete multi-P/UGC collection entries accompany the response. Each selected video is independently revalidated for playback permission; incomplete collection counts return no selectable entries. Returns a short-lived local resource capability, session ID, title, BV/CID, actual declared quality and duration. Exactly one complete MP4 is supported; reject previews, incomplete durations, segmented/DASH resources. No comments, danmaku or next-episode prefetch.
- `playback.suspend/resume/close {session}`: current session only. Suspend stops streaming; resume is explicit and does not itself play. Close destroys the capability.
- `lifecycle.suspend`: cancel current work, clear QR polling state and suspend media. The browser must pause and detach src. Returning visible does not fetch or autoplay.

## Network and credential boundary

Fixed API endpoints on api.bilibili.com: /x/web-interface/nav, /x/web-interface/wbi/search/type, /x/web-interface/view, /x/player/wbi/playurl, /x/web-interface/wbi/index/top/feed/rcmd, /x/polymer/web-dynamic/v1/feed/all. Official QR endpoints on passport.bilibili.com: /x/passport-login/web/qrcode/generate and /poll. QR targets allow only exact HTTPS account.bilibili.com or passport.bilibili.com, without ports/userinfo. WBI uses the public nav image keys and the standard query signature; anonymous nav code -101 is accepted only to read these public keys.

NativeTls with platform certificate verification/system proxy, bounded timeouts and response sizes; redirects are refused. Media hosts are restricted to HTTPS subdomains of bilivideo.com/bilivideo.cn, without explicit ports/userinfo, only from a verified playurl response. Before any media request, choose the first compatible HTTPS address from the same official durl response (primary, then at most eight official backup_url entries); unsupported ports/hosts are never contacted. This is selection before access, never fallback after a failed media request. A CDN receives **only the approved fixed public Referer https://www.bilibili.com/** and a fixed User-Agent. It receives no account Cookie, client Referer or user page information. This narrow normal-protocol exception was explicitly authorized after a no-Referer request returned 403; fixed-Referer Range verification returned 206. Future 401/403 closes the current resource with no retries, identity/device changes or whitelist expansion.

The loopback server accepts fixed media paths, capability tokens, exact Host/Origin and bounded numeric Range requests; no arbitrary URL proxy. At most four handlers and 1 MiB/request, streaming in memory; media is never stored on disk. Resource URLs remain private in-memory session capabilities and must not be logged.

Credentials are Windows generic credentials under LazyBuddy/bilibili/<scope>; default scope production; a non-default LAZYBUDDY_INSTANCE is also an isolation fallback. An isolated host supplies a distinct LAZYBUDDY_CREDENTIAL_SCOPE (lowercase ASCII/digits/hyphen, 1–48 chars). LAZYBUDDY_DATA_DIR is reserved for this service, currently unused; no plaintext credentials are stored there.

## Verification boundary

Anonymous search, 1920×1080 browser frame advancement, decoded nonzero PCM, 2× playback, suspend/read stop and resume-without-autoplay have been verified. QR generation is verified. Real host QR confirmation and secure save/new-process restore have passed. Official recommendations (12), following feed (20+20 with advancing cursor), an 8-P directory/selected second P and a complete 180-video UGC collection/selected CID have passed. The actual frontend component has produced 1920×1080 frames, nonzero PCM, 2× advancement, real second-P playback, hidden read/byte stop and no resume autoplay in headless Edge. Native host visual/audio acceptance is separate and is not inferred from these checks.
