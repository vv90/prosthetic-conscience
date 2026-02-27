---
# **1. Purpose & Scope**

## **1.1 Objective**

Design a cloud-hosted LLM gateway that:
  - Supports multiple workers (GPU nodes)

  - Allows reverse-initiated worker connections

  - Provides API-compatible access to clients

  - Enforces quotas and fairness

  - Supports streaming responses

  - Minimizes data retention

  - Can evolve to full end-to-end encryption without architectural redesign
---

## **1.2 Phase 1 Scope (Relaxed Plaintext)**

- Gateway may see plaintext prompts in memory
- Gateway does not persist prompt content
- Worker processes plaintext
- Standard JSON-based API
- WebSocket support for streaming

---

## **1.3 End Goal (Future State)**

- Gateway never sees plaintext prompts
- Envelope encryption between client and worker
- Gateway acts as blind relay
- Streaming remains supported
- Minimal redesign required from Phase 1

---

## **1.4 Non-Goals**

- Enterprise compliance certification (SOC2/HIPAA/etc.)
- Zero-knowledge cryptographic enforcement (for now)
- Fully anonymous metadata-hiding network
- Multi-tenant billing infrastructure
- Complex RBAC

---

# **2. Actors & Trust Boundaries**

## **2.1 Actors**

### **Client**

- Developer tool / IDE / CLI / sidecar
- Submits inference requests
- Receives streaming responses

### **Gateway**

- Cloud-hosted control plane
- Authenticates users
- Enforces quotas
- Routes requests
- Streams responses

### **Worker**

- GPU node
- Connects outbound to gateway
- Processes inference
- Returns results

---

## **2.2 Trust Model (Phase 1)**

| **Component**    | **Trusted With Plaintext?** |
| ---------------- | --------------------------- |
| Client           | Yes                         |
| Worker           | Yes                         |
| Gateway          | Yes (memory only)           |
| Cloudflare / CDN | No (TLS protected)          |

## **2.3 Trust Model (Future State)**

| **Component**  | **Trusted With Plaintext?** |
| -------------- | --------------------------- |
| Client         | Yes                         |
| Worker         | Yes                         |
| Gateway        | No                          |
| Cloud provider | No                          |

# **3. High-Level Architecture**

```
Client
   │
   │ HTTPS / WebSocket
   ▼
Cloudflare (DDoS, TLS edge)
   │
   ▼
Gateway (cloud VM/container)
   │
   │ Persistent WebSocket
   ▼
Worker (reverse-connected GPU node)
```

# **4. Functional Requirements**

---

## **4.1 Gateway Requirements**

### **Authentication & Authorization**

- API key authentication
- Per-user identity mapping
- Instant key revocation
- Optional IP-based rate policies

---

### **Request Handling**

- Accept OpenAI-compatible requests
- Accept WebSocket and/or HTTP streaming
- Assign unique request ID
- Route request to connected worker
- Support cancellation messages

---

### **Quota & Fairness**

- Per-user RPM limit
- Per-user concurrency limit
- Global concurrency limit
- Per-user daily/monthly token quota
- Per-worker queue limit
- Request body size limit

---

### **Worker Session Management**

- Accept outbound worker connections
- Maintain in-memory registry:
  - Worker ID
  - Active jobs
  - Health state
- Drop inactive workers
- Support heartbeat messages

---

### **Streaming**

- Relay streaming tokens in real time
- Preserve order
- Propagate backpressure
- Detect stream termination
- Handle client disconnects gracefully

---

### **Usage Tracking**

- Track:
  - user_id
  - model
  - tokens_in
  - tokens_out
  - latency
  - status
- Do not store prompt content
- Store usage metadata only

---

### **Failure Handling**

- Timeout enforcement
- Worker disconnect handling
- Partial stream detection
- Job expiration (TTL)
- Graceful error propagation

---

## **4.2 Worker Requirements**

- Initiate persistent outbound connection
- Authenticate to gateway
- Advertise capability metadata (Phase 1 optional)
- Accept job requests
- Enforce:
  - Context limits
  - Token limits
  - Timeout limits
- Support cancellation
- Stream partial results
- Return usage metadata

---

## **4.3 Client / Sidecar Requirements**

- Expose OpenAI-compatible local API
- Handle streaming
- Support cancellation
- In future:
  - Handle envelope encryption
  - Manage session keys
  - Decrypt responses

---

# **5. Protocol Abstractions (Encryption-Ready Design)**

Even in Phase 1, protocol must treat request payload as opaque blob.

### **Required Abstractions**

1. job_id
2. payload (currently plaintext JSON)
3. metadata
4. response_chunk
5. response_complete
6. cancel_request

Gateway must treat payload as opaque object and avoid deep mutation.

This enables future replacement:

```
payload = encrypted_blob
```

Without protocol redesign.

# **6. Streaming Requirements**

- Must support incremental token streaming
- Must preserve ordering
- Must support:
  - Start
  - Chunk
  - End
  - Error
- Must support stream interruption
- Must allow future encrypted chunk mode

Streaming framing must be transport-agnostic (not tied to specific library semantics).

---

# **7. Job Lifecycle & State Model**

## **States**

- RECEIVED
- AUTHENTICATED
- QUEUED
- DISPATCHED
- STREAMING
- COMPLETED
- FAILED
- CANCELLED
- EXPIRED

Gateway maintains in-memory job table.

No job content stored beyond active lifecycle.

---

# **8. Failure & Recovery Semantics**

## **Worker Disconnect Mid-Job**

- Mark job FAILED
- Notify client
- Optionally allow client retry

## **Client Disconnect Mid-Job**

- Send cancellation to worker
- Worker stops generation
- Clean up resources

## **Gateway Restart**

- Active jobs lost
- Clients must retry
- No persistence of job content

---

# **9. Abuse & DoS Controls**

- Max request body size
- Max WebSocket frame size
- Per-IP rate limiting
- Per-user rate limiting
- Max concurrent connections
- Worker queue length limit
- Idle timeout enforcement
- Strict JSON validation

---

# **10. Data Retention & Logging Policy**

## **Allowed to Persist**

- Usage metrics
- Error codes
- Timestamps
- User IDs
- Token counts

## **Forbidden to Persist**

- Prompt text
- Completion text
- Tool arguments
- Full request bodies
- Full response bodies

Logs must redact sensitive fields.

---

# **11. Non-Functional Requirements**

---

## **11.1 Security**

- TLS everywhere
- No prompt persistence
- Minimal log verbosity
- Role separation between gateway admin & worker admin (if different)
- Memory-only processing of prompts

---

## **11.2 Privacy**

- Gateway must not retain content
- Explicit no-retention policy
- Worker only persistent plaintext processor
- Future encryption compatibility

---

## **11.3 Scalability**

- Support multiple workers
- Stateless gateway design (horizontal scaling possible later)
- Worker pool model
- In-memory routing table

---

## **11.4 Observability**

- Health endpoints
- Active job count
- Worker connection count
- Queue depth metrics
- Latency percentiles
- GPU utilization (optional)

---

# **12. Upgrade Path to Full End-to-End Encryption**

## **Target Architecture**

Client → Gateway → Worker

with:

- Envelope encryption
- Worker private key
- Gateway blind relay
- Encrypted response streaming

---

## **Required Changes Later**

### **1. Replace payload field**

```
payload: JSON → payload: encrypted_blob
```

### **2. Add Envelope Fields**

- encrypted_session_key
- nonce
- auth_tag
- worker_key_id

---

### **3. Move Token Counting**

- Worker reports encrypted usage metadata
- Gateway stores encrypted usage

---

### **4. Sidecar Responsibilities Expand**

- Encrypt request
- Decrypt streaming response
- Manage key lifecycle

---

## **What Must Not Change**

- Job lifecycle state model
- Streaming framing abstraction
- Worker reverse connection model
- Authentication layer
- Rate limit model
- Gateway routing logic

Design must ensure these components remain stable.

---

# **13. Open Questions & Tradeoffs**

1. How much worker metadata is gateway allowed to store?
2. Should worker identity be persistent or ephemeral?
3. Should job queues persist encrypted payloads?
4. Should cancellation be best-effort or guaranteed?
5. Should client choose worker or gateway route?
6. Should gateway enforce model allowlists?
7. What is acceptable metadata leakage?

---

# **14. Architectural Tradeoffs**

| **Simpler**               | **More Private**         |
| ------------------------- | ------------------------ |
| Plain JSON forwarding     | Envelope encryption      |
| Central routing           | Client-directed worker   |
| Token counting at gateway | Token counting at worker |
| Minimal protocol          | Cryptographic framing    |

Phase 1 intentionally chooses left column, but builds structure for right column.

---

# **15. Success Criteria for Phase 1**

- Multiple users connected
- Worker reverse-connected
- Streaming works reliably
- Rate limiting enforced
- No prompt persisted
- System survives restart
- No exposed worker ports
- No public direct worker access

---

# **Final Summary**

This specification defines:

- A minimal but structured gateway
- Clear trust boundaries
- Future-ready protocol abstraction
- Streaming-compatible design
- Zero prompt persistence
- Upgrade path toward full E2E encryption

It avoids overengineering while preserving architectural flexibility.
