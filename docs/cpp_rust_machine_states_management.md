## Question 1/3: How should we handle the FFI lifetime and make the active KallistoCore instance accessible to the free-standing FFI functions called by the Rust Admin Server?

Use Handle Pattern (Opaque Pointer) to maintain Stateless FFI Boundary.
Strictly avoid Global State (Option 1) or Singleton (Option 3) to
ensure independent Unit Testing (Inversion of Control) and pave the
way for Multi-tenancy architecture (embedding multiple Kallisto instances
in the future). Memory ownership must be preserved in C++, with Rust
only borrowing the Handle (Borrowing) via FFI.

## Question 2/3: How should the Rust Admin Server (port 8202) be started and stopped during the server's lifecycle?

Choose Option 2. The Admin Server lifecycle must be absolutely controlled
and deterministic (Deterministic) by the C++ Orchestrator through RAII.
Apply LIFO order (Start first, Shut down last) to ensure Graceful
Shutdown. Option 1 is an anti-pattern that leads to resource leaks,
Sudden request drops, and violates Cloud-Native operating standards
when receiving SIGTERM signals from Kubernetes.

## Question 3/3: For the static mock routes on port 8200 (`/v1/sys/mounts`, `/v1/sys/health`, `/v1/sys/seal-status`), where should the C++ routing logic reside?

Choose Option 2. Strictly adhere to Single Responsibility Principle (SRP)
and Open-Closed Principle (OCP) of Clean Architecture. Injecting intercept
logic into HttpHandler::handleRequest (Option 1) would create a God
Class anti-pattern. Instead, design HttpHandler as a pure Request
Dispatcher and delegate the processing of system endpoints to a specialized
SysHandler class, which is encapsulated and tested independently.