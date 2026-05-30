    Checking naughtian-kallisto v1.0.0 (/home/stella/workspace/naughtian-kallisto)
warning: this `if` statement can be collapsed
  --> src/server/http_handler.rs:41:5
   |
41 | /     if let Ok(root) = sonic_rs::from_slice::<Value>(body) {
42 | |         if let Some(arr) = root.pointer(sonic_rs::pointer!["versions"]).and_then(|v| v.as_array()) {
43 | |             for item in arr.iter() {
44 | |                 if let Some(num) = item.as_u64() {
...  |
49 | |     }
   | |_____^
   |
   = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if
   = note: `#[warn(clippy::collapsible_if)]` on by default
help: collapse nested if block
   |
41 ~     if let Ok(root) = sonic_rs::from_slice::<Value>(body)
42 ~         && let Some(arr) = root.pointer(sonic_rs::pointer!["versions"]).and_then(|v| v.as_array()) {
43 |             for item in arr.iter() {
...
47 |             }
48 ~         }
   |

warning: this `if` statement can be collapsed
  --> src/engine/kv_engine.rs:49:13
   |
49 | /             if key.starts_with(b"m:") {
50 | |                 if let Ok(path_str) = std::str::from_utf8(&key[2..]) {
51 | |                     path_index_clone.insert_path_if_absent(path_str);
52 | |                 }
53 | |             }
   | |_____________^
   |
   = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if
help: collapse nested if block
   |
49 ~             if key.starts_with(b"m:")
50 ~                 && let Ok(path_str) = std::str::from_utf8(&key[2..]) {
51 |                     path_index_clone.insert_path_if_absent(path_str);
52 ~                 }
   |

warning: redundant closure
   --> src/engine/kv_engine.rs:178:52
    |
178 |         let meta = self.read_raw_optimistic(&mkey, |data| Self::deserialize_metadata(data))?;
    |                                                    ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ help: replace the closure with the associated function itself: `Self::deserialize_metadata`
    |
    = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#redundant_closure
    = note: `#[warn(clippy::redundant_closure)]` on by default

warning: redundant closure
   --> src/engine/kv_engine.rs:212:55
    |
212 |         let payload = self.read_raw_optimistic(&vkey, |data| Self::deserialize_payload(data))?;
    |                                                       ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ help: replace the closure with the associated function itself: `Self::deserialize_payload`
    |
    = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#redundant_closure

warning: this `if` statement can be collapsed
   --> src/engine/kv_engine.rs:228:9
    |
228 | /         if let Some(expected_cas) = cas {
229 | |             if meta.current_version != expected_cas {
230 | |                 return Err(EngineError::CasMismatch {
231 | |                     expected: expected_cas,
...   |
235 | |         }
    | |_________^
    |
    = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if
help: collapse nested if block
    |
228 ~         if let Some(expected_cas) = cas
229 ~             && meta.current_version != expected_cas {
230 |                 return Err(EngineError::CasMismatch {
...
233 |                 });
234 ~             }
    |

warning: you seem to be trying to use `match` for destructuring a single pattern. Consider using `if let`
   --> src/engine/kv_engine.rs:410:9
    |
410 | /         match queue.dequeue() {
411 | |             Ok(op) => {
412 | |                 dequeued = true;
413 | |                 match op {
...   |
422 | |             Err(_) => {}
423 | |         }
    | |_________^
    |
    = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#single_match
    = note: `#[warn(clippy::single_match)]` on by default
help: try
    |
410 ~         if let Ok(op) = queue.dequeue() {
411 +             dequeued = true;
412 +             match op {
413 +                 AsyncOp::Put { key, value } => {
414 +                     batch.push(BatchOp::Put { key, value });
415 +                 }
416 +                 AsyncOp::Delete { key } => {
417 +                     batch.push(BatchOp::Delete { key });
418 +                 }
419 +             }
420 +         }
    |

warning: you should consider adding a `Default` implementation for `EngineRegistry`
  --> src/engine/engine_registry.rs:12:5
   |
12 | /     pub fn new() -> Self {
13 | |         Self {
14 | |             engines: ArcSwap::from_pointee(HashMap::new()),
15 | |             write_lock: parking_lot::Mutex::new(()),
16 | |         }
17 | |     }
   | |_____^
   |
   = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#new_without_default
   = note: `#[warn(clippy::new_without_default)]` on by default
help: try adding this
   |
11 + impl Default for EngineRegistry {
12 +     fn default() -> Self {
13 +         Self::new()
14 +     }
15 + }
   |

warning: `Vec<T>` is already on the heap, the boxing is unnecessary
 --> src/engine/btree_index.rs:7:22
  |
7 |     pub child_nodes: Vec<Box<Node>>,
  |                      ^^^^^^^^^^^^^^ help: try: `Vec<Node>`
  |
  = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#vec_box
  = note: `#[warn(clippy::vec_box)]` on by default

warning: this expression creates a reference which is immediately dereferenced by the compiler
  --> src/engine/btree_index.rs:74:42
   |
74 |             self.collect_paths_recursive(&node.child_nodes.last().unwrap(), paths);
   |                                          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ help: change this to: `node.child_nodes.last().unwrap()`
   |
   = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#needless_borrow
   = note: `#[warn(clippy::needless_borrow)]` on by default

warning: redundant pattern matching, consider using `is_ok()`
   --> src/engine/lock_free_queue.rs:115:19
    |
115 |         while let Ok(_) = self.dequeue() {}
    |         ----------^^^^^----------------- help: try: `while self.dequeue().is_ok()`
    |
    = note: this will change drop order of the result, as well as all temporaries
    = note: add `#[allow(clippy::redundant_pattern_matching)]` if this is important
    = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#redundant_pattern_matching
    = note: `#[warn(clippy::redundant_pattern_matching)]` on by default

warning: this `if` statement can be collapsed
   --> src/storage/async_flusher.rs:158:5
    |
158 | /     if !batch.is_empty() {
159 | |         if let Err(e) = rocksdb.apply_batch(&batch) {
160 | |             eprintln!("[AsyncFlusher] Final drain flush error: {}", e);
161 | |         }
162 | |     }
    | |_____^
    |
    = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#collapsible_if
help: collapse nested if block
    |
158 ~     if !batch.is_empty()
159 ~         && let Err(e) = rocksdb.apply_batch(&batch) {
160 |             eprintln!("[AsyncFlusher] Final drain flush error: {}", e);
161 ~         }
    |

warning: use of a disallowed method `std::thread::Builder::spawn`
  --> src/event/worker.rs:33:22
   |
33 |                     .spawn(move || {
   |                      ^^^^^
   |
   = note: Wrapper function `<std::thread::Builder as tikv_util::sys::thread::StdThreadBuildWrapper>::spawn_wrapper`
           should be used instead, refer to https://github.com/tikv/tikv/pull/12442 for more details.
           
   = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#disallowed_methods
   = note: `#[warn(clippy::disallowed_methods)]` on by default

warning: `naughtian-kallisto` (lib) generated 12 warnings (run `cargo clippy --fix --lib -p naughtian-kallisto -- ` to apply 9 suggestions)
    Checking control_plane v1.0.0 (/home/stella/workspace/naughtian-kallisto/components/kallisto_cluster)
warning: manual implementation of `.is_multiple_of()`
   --> benches/storage_bench.rs:220:16
    |
220 |             if i % 20 == 0 {
    |                ^^^^^^^^^^^ help: replace with: `i.is_multiple_of(20)`
    |
    = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#manual_is_multiple_of
    = note: `#[warn(clippy::manual_is_multiple_of)]` on by default

warning: unused variable: `pool`
  --> tests/test_phase4.rs:26:9
   |
26 |     let pool = WorkerPool::spawn(1, data_port, state.clone());
   |         ^^^^ help: if this is intentional, prefix it with an underscore: `_pool`
   |
   = note: `#[warn(unused_variables)]` (part of `#[warn(unused)]`) on by default

warning: expected a type, found an associated function
   --> /home/stella/workspace/naughtian-kallisto/clippy.toml:96:1
    |
 96 | / [[disallowed-types]]
 97 | | path = "openssl::cipher::Cipher::fetch"
 98 | | reason = """
 99 | | When a Some(...) value was passed to the properties argument of openssl::cipher::Cipher::fetch, \
100 | | a use-after-free would result. See RUSTSEC-2025-0022
101 | | """
    | |___^
    |
    = help: add `allow-invalid = true` to the entry to suppress this warning

warning: expected a type, found an associated function
   --> /home/stella/workspace/naughtian-kallisto/clippy.toml:102:1
    |
102 | / [[disallowed-types]]
103 | | path = "openssl::md::Md::fetch"
104 | | reason = """
105 | | When a Some(...) value was passed to the properties argument of openssl::md::Md::fetch, \
106 | | a use-after-free would result. See RUSTSEC-2025-0022
107 | | """
    | |___^
    |
    = help: add `allow-invalid = true` to the entry to suppress this warning

warning: `naughtian-kallisto` (bench "storage_bench") generated 1 warning (run `cargo clippy --fix --bench "storage_bench" -p naughtian-kallisto -- ` to apply 1 suggestion)
warning: `naughtian-kallisto` (test "test_phase4") generated 3 warnings (run `cargo clippy --fix --test "test_phase4" -p naughtian-kallisto -- ` to apply 1 suggestion)
warning: `naughtian-kallisto` (lib test) generated 12 warnings (12 duplicates)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.04s
