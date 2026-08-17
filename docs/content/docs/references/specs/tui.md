## Proxy Fleet Status Screens

1. Health (Default): Displays upstream status, token TTL countdown (critical to prevent silent failures), and cache hit rate.
2. Request Diagnostics: Shows recent requests with SO_PEERCRED (identifying the exact requesting process), hit/miss status, age, and failure reasons.
3. Freshness: Shows TTL configuration, median/oldest secret age, and event stream connection status.
4. Exposure: Shows the exact amount of plaintext in memory vs the configured ceiling.

## Controlplane Mode Screens

1. Cluster: Raft cluster status (leader, peer lag).
2. Durability (Default): Shows storage usage, Raft log unpurged entries, and crucially, the last snapshot time and its verification status (kallisto verify-snapshot).
3. Structure: Displays mount points and policies (no secret values).
4. Recent Writes: Audit log of who wrote to which path and when.
5. Risk Debt: A unique screen showing secrets that have never been read or updated for extended periods, highlighting unused secrets that pose pure risk.

## Security Considerations

1. To protect the organization's secret catalog in multi-tenant environments, a --redact-paths flag will be available to only show mount prefixes instead of full paths. TUI access itself will generate an audit log entry.
