---
title: "Vault-compatible KV Engine"
linkTitle: "Vault KV Engine"
weight: 1
---

Kallisto includes a KV (Key-Value) engine that is compatible with **HashiCorp Vault's KV Engine v2 API**. This allows you to use existing Vault clients and automation tools with minimal configuration changes.

The KV engine provides:
-   **Soft-deletes** for secrets
-   **Versioning** for secrets
-   **Check-and-set** operations for conditional updates
-   **Mount-based routing** for multi-tenancy

### 1. Mounting the KV Engine

To use the KV engine, you must first mount it. You can mount it at the default path `secret`, or any other path you prefer.

```bash
# Mount the KV engine at the default path 'secret'
curl -X POST http://localhost:8200/v1/sys/mounts/secret \
  -d '{"type":"kv"}'

# Mount the KV engine at a custom path 'kv'
curl -X POST http://localhost:8200/v1/sys/mounts/kv \
  -d '{"type":"kv"}'
```

### 2. Using the KV Engine

Once mounted, you can use the KV engine just like you would with Vault. See [HTTP API (Vault KV-v2 Compatible)](vault-kv2.md) for detailed usage examples.

### 3. Unmounting the KV Engine

To remove the KV engine, you can unmount it:

```bash
curl -X DELETE http://localhost:8200/v1/sys/mounts/secret
```

### 4. Configuration

You can configure the KV engine using the standard Vault configuration options. For more information about the KV engine configuration, see the [Vault KV Engine documentation](https://developer.hashicorp.com/vault/docs/secrets/key-value/config).

> **Note:** For more details on the KV engine, please refer to [Vault KV Engine documentation](https://developer.hashicorp.com/vault/docs/secrets/key-value/config).