# Kallisto demo

```bash
docker compose -f docker-compose.demo.yml --env-file .env.demo up
```

Wait ~20s for init to finish, then:

```bash
# Is Kallisto serving?
curl -s http://127.0.0.1:8200/v1/sys/health | python3 -m json.tool

# Read a secret
curl -s http://127.0.0.1:8200/v1/secret/data/app/database | python3 -m json.tool

# List secrets under app/
curl -s http://127.0.0.1:8200/v1/secret/metadata/app/ | python3 -m json.tool
```

MinIO UI at **http://localhost:9001** (demo-access-key / demo-secret-key). Open the
`kallisto-demo` bucket to see `demo.kal` — the AES-256-GCM file Kallisto pulled.

## What's running

```
MinIO (:9000 S3, :9001 UI)
  → kallisto-init [one-shot]
      creates bucket, seals plain.demo.json → demo.kal, uploads
  → kallisto (:8200)
      polls bucket every 5s, serves KV-v2 reads
```

Kallisto refreshes every 5 seconds. `"sealed": false` in the health response
means it loaded the file successfully. `kallisto_file_version` is the content
version inside the sealed file.

## Credentials

`.env.demo` contains throwaway values committed on purpose. Generate a new seal key with:

```bash
cargo run -p kallisto-ctl -- gen-key
```

## Tear down

```bash
docker compose -f docker-compose.demo.yml --env-file .env.demo down -v
```
