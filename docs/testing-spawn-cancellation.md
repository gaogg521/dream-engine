# Spawn cancellation acceptance

`AgentSpawner::spawn_parallel` owns its child futures. Dropping the call or
aborting its parent task cancels all children. Results remain in input order;
a child panic becomes an error result without cancelling successful siblings.
The provider stream worker also stops when its event receiver closes, including
while the upstream is silent or while an empty-stream retry is waiting.

The default suite covers cancellation before provider response headers, dropping
and aborting the parent, per-child panic isolation, silent response ownership,
receiver closure before the worker starts, and retry cancellation.

For transport acceptance, start the deterministic loopback SSE fixture in a
separate terminal:

```sh
python crates/dream-engine-agent/tests/fixtures/spawn_cancel_upstream.py
```

Then run the ignored manual test explicitly:

```sh
cargo test -p dream-engine-agent --test spawn_http_live -- --ignored --nocapture
```

This uses the real OpenAI provider and SDK against five quiet HTTP streams.
It cancels the parent while the test host remains alive and requires all five
upstream connections to close within three seconds, with no replacement request.
Expected final counts are `started=5`, `closed=5`, `active=0`, `maxActive=5`.
It requires no cloud credentials or paid model calls. Stop the fixture terminal
afterwards. Its event log is written to the OS temporary directory, or the path
in `SPAWN_CANCEL_EVENT_LOG`; `/status` exposes only controlled test counters.

This change does not alter the existing policy allowing explicitly launched
background descendants of ordinary `ExecCommand` to survive command completion.
It does not add MCP inheritance to spawned agents.
