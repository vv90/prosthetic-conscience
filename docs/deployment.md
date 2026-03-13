# Deployment Guide

Deploy the gateway on a public server with TLS, then connect workers and clients over the internet.

## Architecture

```
Client (anywhere)
  |
  | HTTPS / WSS
  |
  ▼
┌─────────────────────────┐
│  TLS termination        │  (App Platform, Caddy, or similar)
│    │                    │
│    ▼                    │
│  Gateway (:3000)        │
│    ▲                    │
└────│────────────────────┘
     │ WSS
     │
Worker (GPU machine)
  outbound connection (no inbound ports needed)
```

## Option A: DigitalOcean App Platform (managed)

App Platform handles TLS, scaling, and restarts. No Caddy needed.

### Setup

1. Create an App on App Platform, source from GHCR:
   - **Registry**: `ghcr.io`
   - **Image**: `<your-user>/prosthetic-conscience`
   - **Tag**: `latest`

2. Configure the component:
   - **HTTP port**: `3000`
   - **Health check**: TCP on port `3000`

3. Set environment variables:
   - `PC_AUTH_TOKEN` = `<openssl rand -hex 32>` (mark as secret)

4. Deploy. App Platform provides a `*.ondigitalocean.app` domain with HTTPS automatically. Custom domains can be added in settings.

5. Verify:

```bash
# Should return 401 (no auth)
curl https://your-app.ondigitalocean.app/v1/chat/completions

# Should return an SSE error about no workers (proves auth + TLS work)
curl -N -H "Authorization: Bearer <your-token>" \
  -H "Content-Type: application/json" \
  -d '{"model":"test","messages":[{"role":"user","content":"hello"}],"stream":true}' \
  https://your-app.ondigitalocean.app/v1/chat/completions
```

### Updating

CI pushes a new image to GHCR on every push to `main`. In App Platform, enable auto-deploy from the registry, or trigger a manual redeploy.

## Option B: Self-hosted with Docker Compose + Caddy

For a raw VPS (DigitalOcean droplet, Hetzner, etc.) where you manage everything yourself. Caddy provides automatic TLS via Let's Encrypt.

### Prerequisites

- A VPS with Docker and Docker Compose installed
- A domain name with DNS pointing to the VPS IP
- The gateway Docker image in GHCR (built by CI on push to main)

### Setup

1. SSH into the server.

2. Copy `docker-compose.yml` and `Caddyfile` to the server (or clone the repo).

3. Create a `.env` file:

```bash
echo "PC_AUTH_TOKEN=$(openssl rand -hex 32)" > .env
echo "DOMAIN=gateway.yourdomain.com" >> .env
echo "GITHUB_REPOSITORY=youruser/prosthetic-conscience" >> .env
```

4. Log in to GHCR (needed to pull private images):

```bash
echo $GITHUB_PAT | docker login ghcr.io -u USERNAME --password-stdin
```

5. Start the stack:

```bash
docker compose up -d
```

Caddy will automatically obtain a TLS certificate from Let's Encrypt.

6. Verify:

```bash
# Should return 401 (no auth)
curl https://gateway.yourdomain.com/v1/chat/completions

# Should return an SSE error about no workers (proves auth + TLS work)
curl -N -H "Authorization: Bearer <your-token>" \
  -H "Content-Type: application/json" \
  -d '{"model":"test","messages":[{"role":"user","content":"hello"}],"stream":true}' \
  https://gateway.yourdomain.com/v1/chat/completions
```

### Updating

```bash
docker compose pull
docker compose up -d
```

## Worker Setup

On the machine with the GPU:

1. Start the inference backend (llama-server or any OpenAI-compatible endpoint):

```bash
llama-server -m model.gguf --port 8080
```

2. Run the worker agent:

```bash
pc-worker \
  --gateway-url wss://your-gateway-domain/ws/worker \
  --inference-url http://127.0.0.1:8080 \
  --auth-token <same-token-as-gateway>
```

The worker connects outbound to the gateway. No inbound ports or firewall changes needed.

## Client Usage

Any OpenAI-compatible client works. Point it at the gateway:

```bash
curl -N \
  -H "Authorization: Bearer <your-token>" \
  -H "Content-Type: application/json" \
  -d '{"model":"test","messages":[{"role":"user","content":"hello"}],"stream":true}' \
  https://your-gateway-domain/v1/chat/completions
```

Or with the OpenAI Python client:

```python
from openai import OpenAI

client = OpenAI(
    base_url="https://your-gateway-domain/v1",
    api_key="<your-token>",
)

response = client.chat.completions.create(
    model="test",
    messages=[{"role": "user", "content": "hello"}],
    stream=True,
)

for chunk in response:
    print(chunk.choices[0].delta.content, end="", flush=True)
```

## Validation Checklist

- [ ] Worker connects over WSS, shows "connected to gateway" in logs
- [ ] Client streams tokens through the gateway
- [ ] Kill worker mid-stream: client gets error + done
- [ ] Kill inference backend: worker sends error, client gets error
- [ ] Kill gateway: worker reconnects with backoff
- [ ] Wrong/missing token: 401 rejected
