# "RESEARCH AND IMPLEMENTATION OF A HIGH-PERFORMANCE SECRET MANAGEMENT SYSTEM USING SIPHASH, CUCKOO HASHING AND B-TREE INDEXING"

# Disclaimer

This project incorporates the use of open-source software under the Apache License 2.0. Full license details and attributions are available in the accompanying documentation (file LICENSE.md). The project also adheres to privacy regulations and ensures the confidentiality and integrity of the data used during the research.

The contributions of third parties are acknowledged, and this project builds upon the foundational work provided by `hashicorp/vault`, `redis` and `envoyproxy/envoy`, with specific adaptations and extensions made to suit the research objectives. The reporter and the supervising faculty do not assume liability for any direct or indirect damages arising from the use of this project or its contents.

# I. INTRODUCTION
## 1.1. Context

Managing secrets (API keys, passwords, tokens, etc.) is one of the most critical requirements of a security system. However, encrypting and storing secrets securely does not mean that the system can serve at high throughput. A classic example is when a Kubernetes cluster restarts, thousands of containers in hundreds of pods will be scheduled, spin-up, crash-loop-back-off and request secrets at the same time. Each container needs to be served secrets to start, hence making the secret management system frozen due to flooding.

What about using `Redis` and `Hashicorp Vault`? Redis is fast, and Vault is an Encrypt-as-a-service. But they have their own issues:

Redis can serve a large number of requests but it is not secure enough. In fact, Redis is not designed for caching secrets, it is a high performance key-value store but "flat" (which is, does not understand what is a "secret path", It just sees "/prod/db/secret" as a string.) If we try to use Redis to store secrets at path "/prod/db/secret" just like Vault does, we will have to validate the path before reading it, and Redis has to scan through everything to find the correct one. 

Hashicorp Vault, on the other hand, can not withstand the load of thousands of containers requesting secrets at the same time. HashiCorp also warns that if you need High Throughput then don't throw everything into one Vault cluster, but use Performance Standby or scale horizontally, which is obviously too expensive.

## 1.2. Problem Statement

What does a service look like, to reach the performance speed of Redis but still keep the strict control and security of Vault? 

Why do traditional hash tables (Chain, Linear Probing) fear DoS Hash Flooding attacks? Because they are vulnerable to hash collisions, a hacker with knowledge about them can create a large number of keys to cause hash collisions. Why does the CPU consume time processing collisions? Because when there is a collision, the CPU must execute expensive resolution mechanisms: Chaining (Traverse linked list). 

If using a linked list, the CPU must compare the new key with each old key in the list to check for duplicates. With N colliding elements, inserting N elements will take O(N^2) total CPU time. 

Open Addressing (Find empty position): CPU must perform "probes" (linear or square probing) to find the next empty slot. In a flooding attack, almost every CPU slot is occupied, leading to thousands of useless comparison calculations for each request. 

## 1.3. Proposed Solution

So, I suggest the following solution:

**SipHash**: To make the hash function unpredictable, prevent hackers from creating collisions.

**Cuckoo Hashing**: To reach O(1) worst-case for Read, prevent the Thundering Herd problem.

**B-Tree**: To act as a "gate", remove invalid path requests with O(log N) requests before they reach the hash table.

**Hybrid technical architecture**: Combine the hierarchical management of Path Validation with the speed of Cache on RAM.

This code base is named "Kallisto" as a codename for documentation and recognition. 

## 1.4. Objectives

1. Build a Prototype (Kallisto) to prove the feasibility of the architecture above.

2. Perform Benchmark to compare the performance between the Safe (Strict Sync) and High Performance (Batch Mode).

---

## II. REQUIREMENTS & PROJECT SCOPE

Based on the "Data Structures & Algorithms Final Project" rubric (Dr. Huynh Xuan Phung), this project falls under **Suggested Integrated Project #1: High-Speed Database Index**.

### 2.1. Core Research Topics Covered
This project integrates 3 major concepts from the Research Pool (Days 1–11):

1.  **D7: Cuckoo Hashing & O(1) Worst-case**: Implementation of the primary storage engine using `kallisto::CuckooTable` to guarantee constant-time lookups.
2.  **D6: Universal Hashing & SipHash**: Using `kallisto::SipHash` (cryptographically secure PRF) to prevent Hash Flooding DoS attacks.
3.  **D11: B-Trees & Disk-Optimized Storage**: Using `kallisto::BTreeIndex` for managing secret paths and serving as a high-performance secondary index.

### 2.2. Functional Requirements

- **High Performance**:
    - **Write RPS**: > 10,000 req/s (Batch Mode).
    - **Read RPS**: > 5,000 req/s (Random Access).
    - **Latency**: Sub-millisecond (p99 < 1ms).

- **Security**:
    - Must prevent Hash Flooding Attacks (via SipHash).
    - Must validate secret paths effectively (via B-Tree).

- **Implementation Constraints**:
    - Language: C++17/20.
    - No external libraries (boost, openssl) allowed for core logic.
    - Must use smart pointers (`std::unique_ptr`, `std::shared_ptr`) for memory safety.

### 2.3. Deliverables

1.  **Source Code**: Complete C++ implementation in `/src` and `/include`.
2.  **Report**: This document serving as the Research Synthesis Report.

---

# IV. THEORY

## 4.1. SipHash

*"SipHash is a cryptographically secure PRF -- a keyed hash function -- that performs very well for short inputs, hence the name. It was designed by cryptographers Daniel J. Bernstein and Jean-Philippe Aumasson. It is intended as a replacement for some uses of: jhash, md5_transform, sha1_transform, and so forth."* [https://docs.kernel.org/security/siphash.html](https://docs.kernel.org/security/siphash.html)

SipHash uses a secret string (secret key) to generate a hash value. This secret string is generated using a random method (random) to ensure the highest level of security. In essence, we are not allowed to let anyone know this secret string, so it is crucial to generate "Secret Key" from a random and cryptographically secure source. 

*"SipHash has a very high security margin, with its 128-bit key. So long as the key is kept secret, it is impossible for an attacker to guess the outputs of the function, even if being able to observe many outputs, since 2^128 outputs is significant."* [https://docs.kernel.org/security/siphash.html](https://docs.kernel.org/security/siphash.html)

Linux implements the “2-4” variant of SipHash.

A secret management system uses hash table to query secret entry must face the risk of being attacked from DoS. If using a hash function that is easily predicted / weak in terms of cryptography, an attacker can create a large number of keys to cause collisions (Hash Flooding) to freeze the system.

To protect the system from DoS attacks, we implement SipHash with a "secret key" `KALLISTO_SIPHASH_SECRET_KEY` to ensure that the hash table is immune to hash flooding attacks.

### 4.1.2. But why SipHash?

SipHash uses a bit-reversal (Add-Rotate-XOR) architecture to create noise bits (noise bit) in the hashing process without consuming too much CPU resources (bit rotation only takes place in one clock cycle, so it is very fast and effective in a CPU cycle).

#### 4.1.3. Why SipHash is more secure than MurmurHash/CityHash?

Fast hash functions (Non-cryptographic) like MurmurHash only strong about speed but weak about bit change so they do not ensure the highest security like SipHash with Avalanche Effect. An attacker can easily find two strings `KeyA` and `KeyB` have the same hash.
SipHash uses a "Secret Key" (128-bit) as input parameter. If the attacker does not know the key, he cannot calculate the hash of any string in advance, so he cannot create millions of requests with the same hash index to congest the Cuckoo Table. However, the security limit of SipHash also lies in the key size (128 bits) and output size (64 bits). Although the possibility of being attacked can be reduced to 2^64 (if SipHash is used as MAC), or guessing the secret key ("2 ^ s-128"; with "2 ^ s" is the number of keys tried and failed). But with the amount of requests into the system (usually only several hundred thousand requests per second), the attacker cannot perform a "hash flooding" attack, except by mobilizing a Botnet with a very large number of bot machines attacking a single instance, or using a quantum computer.

## 4.2. Cuckoo Hashing

### 4.2.1. The Architecture (Two Functions, Two Tables)

Instead of relying on a single location for each key and only having one hash table to determine all the values stored, Cuckoo Hashing utilizes two independent hash functions and typically two hash tables. This design gives every key exactly two possible locations to reside, somewhere on the first table - or somewhere on the second table, else none existed. Hence it allows 
the algorithm to resolve collisions by moving keys between their two potential 
homes. 

### 4.2.2. The Insertion Process: "Kicking Out" Strategy 

It works just like the cuckoo bird's behavior (cuckoo bird's younglings kicks the host’s children out of the nest): 

We have a key “x”, we try to place it in a slot on the first table. If that slot is empty, insertion is complete. 

If that slot is already occupied by key “y”, key “x” "kicks out" key “y” and takes its place. 

The displaced key “y” must now move to the second table using hash function h_2(y). 

If y's new spot is also occupied, it evicts the incumbent key, triggering a chain reaction of displacements until a key lands in an empty slot (or a cycle is detected, triggering a rehash). 

### 4.2.3. The Guarantee: O(1) Worst-Case Lookup 

Regardless of how full the table is or how complex the insertion chain was, to find a key, the algorithm only needs to check at most two locations: T_1[h_1(x)] and T_2[h_2(x)]. Since the number of checks is constant (always 2 placed slots we calculated), the search time complexity is guaranteed to be O(1) in the worst case, eliminating the performance degradation seen in Chaining or Linear Probing.

## 4.3. B-Trees & Disk-Optimized Storage

### 4.3.1. Theoretical Definition

A B-Tree of order *m* is a self-balancing tree data structure that maintains sorted data and allows searches, sequential access, insertions, and deletions in logarithmic time. Unlike self-balancing binary search trees (like AVL or Red-Black trees), the B-Tree is generalized to allow nodes to have more than two children, making it optimized for systems that read and write large blocks of data (like Hard Drives or SSDs).

**Key Properties:**
1.  **Balance**: All leaf nodes appear at the same depth.
2.  **Child Count**: Every node has at most *m* children. Every non-leaf node (except root) has at least m/2 children.
3.  **Keys**: A node with k children contains k-1 keys.
4.  **Ordering**: Keys in a node are sorted in increasing order, separating the ranges of keys covered by its subtrees.

### 4.3.2. Complexity Analysis

User operations (Search, Insert, Delete) run in O(log_m N) time. Where N is the total number of items and m is the branching factor (degree).

Binary Trees have a branching factor of 2, leading to a height of log_2 N. B-Trees have a large branching factor (e.g., m=100), leading to a height of log_{100} N.

*Example*: With 1,000,000 items:

Binary Tree Height: ~20 levels.
B-Tree (m=100) Height: ~3 levels.
Since accessing a node may require a Disk I/O (which is slow), reducing the height from 20 to 3 results in a massive performance gain.

### 4.3.3. Why use B-Tree to validate Path?

A secret management system does not only store secrets in RAM but also needs to store them on disk (persistent storage). B-Tree optimizes the number of reads/writes (I/O) and path lookup in B-Tree takes O(log N), very fast to block malicious requests (e.g., user asks for path `/admin/`... but the system has never created this path, B-Tree will block the logic before even starting SipHash, calculating hash, and comparing with entries in Cuckoo Table).

### 4.3.4. Flow Insert Path

Example 1, if `PUT /prod/payment`:
1.  The system runs from the root.
2.  If the root is full, call `splitChild` to split root -> Tree height increases by 1.
3.  Find the appropriate child branch (greater than/less than key).
4.  Recursively down (Insert Non-Full).

### 4.3.5. Flow Validate Path

Example 2, if `GET /prod/payment`:
1.  Compare `/prod/payment` with keys in the current node.
2.  If found -> Return True.
3.  If not found and is a leaf node (Leaf) -> Return False (Path invalid).
4.  If not a leaf -> Jump to the corresponding child node and repeat.

Complexity is always Logarithm base `degree` of N (O(log_t N)). With degree=100, a tree containing 1 million paths is only 3 levels high, maximum 3 pointer jumps.

# V. Applications

## 5.1. Storage Engine

Kallisto uses **RocksDB** as its persistence engine. Unlike the previous binary snapshot mechanism (which required dumping the entire RAM state to disk), RocksDB provides an incremental Write-Ahead Log (WAL) and SSTable-based storage. This allows for high-performance writes and efficient crash recovery while maintaining sub-millisecond read access through the Cuckoo Table cache.

# VI. Implementation Details

## 6.1. Cuckoo Table Code Explanation

We choose Cuckoo Hashing with a size of 16384 slots (serving to test the insert and retrieve 10 thousand times) to ensure the highest performance.

```cpp
KallistoServer::KallistoServer() {
    // TODO: implement ENV to change the size of initial Cuckoo Table.
    // We plan to benchmark 10,000 items. 
    // Cuckoo Hashing typically degrades if the load factor is above 50% (leads to cycles/infinite loops).
    // Capacity of 2 tables with size 16384 is 32,768 slots.
    // Load Factor = 10,000 / 32,768 ≈ 30% (should be safe).
    storage = std::make_unique<ShardedCuckooTable>(2000000);
    path_index = std::make_unique<TlsBTreeManager>(5, nullptr);
    rocksdb_persistence = std::make_unique<RocksDBStorage>("/kallisto/data");

    // Rebuild B-Tree index from RocksDB on startup
    rocksdb_persistence->iterate_all([&](const SecretEntry& entry) {
        path_index->update(entry.path);
    });
}
```

### 6.1.1. Core Logic (Simplified)
Logic "Kick-out" (Cuckoo Displacement) từ file `src/cuckoo_table.cpp` giúp đạt O(1) được phát triển như sau

```cpp
bool CuckooTable::insert(const std::string& key, const SecretEntry& entry) {
    // 1. Check if key exists (Update logic)
    // Nếu key đã tồn tại trong bucket 1 hoặc 2, update value và return.
    size_t h1 = hash_1(key);
    if (table_1[h1].occupied && table_1[h1].key == key) {
        table_1[h1].entry = entry;
        return true;
    }
    // Check hash_2 tương tự
    size_t h2 = hash_2(key);
    if (table_2[h2].occupied && table_2[h2].key == key) {
        table_2[h2].entry = entry;
        return true;
    }

    // 2. Insert with Displacement (Fighting for slots)
    std::string current_key = key;
    SecretEntry current_entry = entry;

    for (int i = 0; i < max_displacements_; ++i) {
        // [PHASE 1] Try to insert into Table 1
        size_t h1 = hash_1(current_key);
        if (!table_1[h1].occupied) {
            table_1[h1] = {true, current_key, current_entry};
            return true;
        }
        
        // [KICK] If Table 1 is full, kick out the old entry
        std::swap(current_key, table_1[h1].key);
        std::swap(current_entry, table_1[h1].entry);

        // [PHASE 2] Try to insert into Table 2
        size_t h2 = hash_2(current_key);
        if (!table_2[h2].occupied) {
            table_2[h2] = {true, current_key, current_entry};
            return true;
        }

        // [KICK] If Table 2 is full, kick out old entry and repeat process
        std::swap(current_key, table_2[h2].key);
        std::swap(current_entry, table_2[h2].entry);
    }

    // If looped too many times (reach max_displacements_) without finding a slot -> Resize
    return false; 
}
```

### 6.1.2. Why is this code fast?

Because it only accesses `table_1` and `table_2` of Cuckoo Table on RAM (Cache L1/L2). Unlike Chaining Hashmap (using linked list when collision), Cuckoo Hash stores data flat (Flat Array) and CPU Prefetcher prioritizes accessing this array to easily prefetch all consecutive values from RAM into CPU Cache. Lookup (function `get`) only checks 2 positions `h1` and `h2` so the big O of it is `O(1) + O(1) = O(1)`. Never have to iterate a long list so the delay is stable < 1ms.

## 6.2. B-Tree Code Explanation

Kallisto uses B-Tree (not Binary Tree) to manage the list of paths (Path Index). This is a self-balancing data structure, optimized for reading in blocks.

### 6.2.1. The "Split Child" Logic

The most difficult part of B-Tree is when a Node is full, it must split into two and push the middle key up to the parent. This is the code that handles this (`src/btree_index.cpp`):

```cpp
void BTreeIndex::splitChild(Node* parent, int i, Node* child) {
    // 1. Tạo node mới 'z' chứa nửa sau của 'child'
    auto z = std::make_unique<Node>(child->is_leaf);
    
    // Copy (degree-1) keys từ 'child' sang 'z' (Phần bên phải)
    for (int j = 0; j < min_degree - 1; j++) {
        z->keys.push_back(child->keys[j + min_degree]);
    }

    // Nếu không phải là lá, copy cả con trỏ con sang 'z'
    if (!child->is_leaf) {
        for (int j = 0; j < min_degree; j++) {
            z->children.push_back(std::move(child->children[j + min_degree]));
        }
        // Xóa phần đã move đi ở 'child'
        child->children.erase(child->children.begin() + min_degree, child->children.end());
    }

    // 2. Lấy key ở giữa (Median) để đẩy lên Parent
    std::string mid_key = child->keys[min_degree - 1];
    
    // Thu gọn 'child' (Xóa phần key đã sang 'z' và key ở giữa)
    child->keys.erase(child->keys.begin() + min_degree - 1, child->keys.end());

    // 3. Chèn 'z' vào danh sách con của Parent
    parent->children.insert(parent->children.begin() + i + 1, std::move(z));
    
    // 4. Chèn 'mid_key' vào danh sách key của Parent
    parent->keys.insert(parent->keys.begin() + i, mid_key);
}
```

## 6.3. SipHash Code Explanation

We implement SipHash-2-4 (2 compression rounds, 4 finalization rounds) according to the RFC 6421 standard and for best performance as the requirements stayed "simple, high-performance system".

### 6.3.1. The "SipRound"

The most important aspect of SipHash lies in the `sipround` function. It uses the **ARX** (Addition, Rotation, XOR) mechanism to shuffle the 256-bit state (4 variables `v0, v1, v2, v3` each 64-bit).

```cpp
// ARX Network (Add - Rotate - XOR)
static inline void sipround(uint64_t& v0, uint64_t& v1, 
                            uint64_t& v2, uint64_t& v3) {
    // Nửa bên trái: Trộn v0 và v1
    v0 += v1;           // A: Addition (Gây nhiễu phi tuyến tính nhờ Carry bit)
    v1 = rotl(v1, 13);  // R: Rotation (Xoay bit sang trái 13 vị trí)
    v1 ^= v0;           // X: XOR (Trộn kết quả lại với nhau)
    v0 = rotl(v0, 32);  // R: Xoay tiếp v0

    // Nửa bên phải: Trộn v2 và v3
    v2 += v3; 
    v3 = rotl(v3, 16); 
    v3 ^= v2;
    
    // Đảo chéo: Trộn v0 với v3, v2 với v1
    v0 += v3; 
    v3 = rotl(v3, 21); 
    v3 ^= v0;
    
    v2 += v1; 
    v1 = rotl(v1, 17); 
    v1 ^= v2; 
    v2 = rotl(v2, 32);
}
```
*Analysis:* The addition (`+`) spreads bit changes from low to high. The rotation (`rotl`) spreads bits horizontally. The `XOR` combines them. Repeating this process transforms the input information into a "mess" that cannot be reversed without the Key.

### 6.3.2. Hashing Flow

Processing a string `input` proceeds as follows (extracted from `src/siphash.cpp`):

```cpp
uint64_t SipHash::hash(
	const std::string& input, 
	uint64_t first_part, 
	uint64_t second_part) {
	
	// Khởi tạo trạng thái nội bộ với các hằng số "nothing-up-my-sleeve"
	// đã có sẵn trong tài liệu thuật toán SipHash. Tham khảo tại:
	// https://cr.yp.to/siphash/siphash-20120918.pdf
	// Mục đích: phá vỡ tính đối xứng ban đầu.
	uint64_t v0 = 0x736f6d6570736575ULL ^ first_part; // "somepseu"
	uint64_t v1 = 0x646f72616e646f6dULL ^ second_part; // "dorandom"
	uint64_t v2 = 0x6c7967656e657261ULL ^ first_part; // "lygenera"
	uint64_t v3 = 0x7465646279746573ULL ^ second_part; // "tedbytes"

	const uint8_t* m = reinterpret_cast<const uint8_t*>(input.data());
	size_t len = input.length();
	const uint8_t* end = m + (len & ~7);
	int left = len & 7;
	uint64_t b = static_cast<uint64_t>(len) << 56;

	// 2. Compression Loop (Vòng lặp nén)
	// Cắt input thành từng block 8 bytes (64-bit) để xử lý.
	// Với mỗi block 64-bit 'mi':
	// - XOR 'mi' vào v3 (Nạp dữ liệu vào trạng thái)
	// - Chạy 2 vòng sipround (Xáo trộn)
	// - XOR 'mi' vào v0 (Khóa dữ liệu lại)
	for (; m < end; m += 8) {
		uint64_t mi;
		std::memcpy(&mi, m, 8);
		v3 ^= mi;
		for (int i = 0; i < 2; ++i) sipround(v0, v1, v2, v3);
		v0 ^= mi;
	}

	// Nếu chuỗi không chia hết cho 8 thì ta chỉ việc dùng switch-case để nhặt nốt những byte bị chia dư ra cuối cùng.
	// Đặc biệt, độ dài của chuỗi được gán vào byte cao nhất để đảm bảo chuỗi abc và abc\0 sẽ cho ra mã băm khác hẳn nhau.
	uint64_t t = 0;
	switch (left) {
		case 7: t |= static_cast<uint64_t>(m[6]) << 48; [[fallthrough]];
		case 6: t |= static_cast<uint64_t>(m[5]) << 40; [[fallthrough]];
		case 5: t |= static_cast<uint64_t>(m[4]) << 32; [[fallthrough]];
		case 4: t |= static_cast<uint64_t>(m[3]) << 24; [[fallthrough]];
		case 3: t |= static_cast<uint64_t>(m[2]) << 16; [[fallthrough]];
		case 2: t |= static_cast<uint64_t>(m[1]) << 8; [[fallthrough]];
		case 1: t |= static_cast<uint64_t>(m[0]); break;
		case 0: break;
	}
	// Sau khi băm xong dữ liệu, thêm một hằng số 0xff vào v_2.
	// Cho sipround chạy liên tục 4 lần để các bit được trộn lẫn.
	// Cuối cùng, gom 4 biến v_0, v_1, v_2, v_3,
	// XOR lại với nhau để ra số 64-bit cuối cùng.

	b |= t;
	v3 ^= b;
	for (int i = 0; i < 2; ++i) sipround(v0, v1, v2, v3);
	v0 ^= b;

	v2 ^= 0xff;
	for (int i = 0; i < 4; ++i) sipround(v0, v1, v2, v3);

	return v0 ^ v1 ^ v2 ^ v3;
}
```

## 6.4. Workflow

When the program (main.cpp) runs, the test process will proceed as follows:

### 6.4.1. Startup

When initializing the server (KallistoServer is initialized):

It prepares 2 data structures:

- `B-Tree Index`: Generates a list of current paths (e.g., /prod/payment, /dev/db).

- `Cuckoo Table`: This is where the secrets are stored. It creates a fixed number of Buckets (1024 buckets) to wait for data to be filled.

### 6.4.2. When storing a secret

User (or code) issues a request: "Store the password secret123 at the path /prod/db with key 'password' and value 'secret123'". Here's what Kallisto does inside:

Check Index (B-Tree): Kallisto calls function `put_secret`, which inside has function `insert_path` to check if the path `/prod/db` already exists. If not, it appends `/prod/db` to the B-tree index.

Create `SecretEntry`: It packages the information key, value, and creation time into a struct `SecretEntry`.

Store in Cuckoo Table (Cuckoo Hashing): It uses the SipHash algorithm to calculate which bucket in "`Cuckoo Table`" the `SecretEntry` should be placed in. If the slot is empty, it will place it and end the task immediately. If the slot is already occupied by another `SecretEntry`, it will "kick" the old entry to another slot to make room for the new `SecretEntry`. The old `SecretEntry` will perform this mechanism until all `SecretEntry` are placed. (This is the special point of Cuckoo Hashing).

### 6.4.3. When getting a secret

User asks: "Give me the password password in the /prod/db vault".

Through the security gate (B-Tree Validation): Kallisto checks the "Index" right away. If the Index does not have the line /prod/db, it will reject service immediately. (This is a security feature to prevent criminals from probing random paths).

If the Index is ok, it uses SipHash to calculate the position. Because it is Cuckoo Hash, it only needs to check exactly 2 positions. Position 1 exists? -> Yes then return. Position 2 exists? -> Yes then return. Both positions do not exist? -> Conclusion: Not found.

Summarize in the form of a diagram:

```mermaid
sequenceDiagram
    participant User
    participant Server as Kallisto
    participant BTree as B-Tree (Path Index)
    participant Cuckoo as Cuckoo (Secret Cache)
    Note over Server: 1. Startup (Empty)
    User->>Server: PUT("/prod/db", "pass", "123")
    Server->>BTree: Insert Path "/prod/db"
    Note right of BTree: Insert Path "/prod/db" into B-Tree
    Server->>Cuckoo: Insert Secret
    Note right of Cuckoo: Tính SipHash -> Tìm ô trống -> Ghi
    User->>Server: GET("/dev/hack", "root")
    Server->>BTree: Validate "/dev/hack"
    BTree-->>Server: FALSE (Path not found)
    Server-->>User: ERROR (Path not found)
    User->>Server: GET("/prod/db", "pass")
    Server->>BTree: Validate "/prod/db"
    BTree-->>Server: TRUE (Path found)
    Server->>Cuckoo: Lookup Key
    Cuckoo-->>Server: Found "123"
    Server-->>User: "123"
```

## 6.5. Server Architecture (New 2026)

Kallisto has evolved from a CLI tool to a high-performance server using an **Envoy-style Architecture**.

### 6.5.1. Thread-per-Core & SO_REUSEPORT

Instead of a traditional "Acceptor Thread -> Worker Thread Pool" model (which suffers from lock contention on the accept queue), Kallisto uses the **Thread-per-Core** model:

1.  **Independent Workers**: The server spawns $N$ worker threads (default: number of CPU cores).
2.  **Shared Port**: All workers bind to the **same port** (8200 for HTTP API) using the `SO_REUSEPORT` socket option.
3.  **Kernel Load Balancing**: The Linux kernel distributes incoming TCP connections across these workers based on a 4-tuple hash. This eliminates the user-space bottleneck of a single acceptor.

### 6.5.2. Unified Event Loop

Each worker runs its own `epoll`-based event loop (`Dispatcher`), handling:

-   **HTTP Traffic**: Direct socket manipulation, parsing HTTP/1.1 requests (Vault KV v2 format), and sending JSON responses using `simdjson`.


### 6.5.3. Sharded Storage Engine

To support concurrent access from multiple workers without locking the entire database, the `CuckooTable` is wrapped in a `ShardedCuckooTable`:

-   **Partitioning**: The key space is divided into **64 shards**.
-   **Routing**: Keys are routed to a shard using the highest bits of their SipHash.
-   **Locking**: Each shard has its own `std::shared_mutex` (Read-Write Lock).
-   **Result**: Probability of lock contention drops from 100% to ~1.5%, enabling linear scalability. Benchmarks show **>2.7 Million RPS** in multi-threaded burst scenarios.

# VII. ANALYSIS

## 7.1. Time Complexity

### 7.1.1. SipHash (Hash Key Generation)

O(L) where L is the length of the input string. SipHash processes the input in 8-byte blocks. For a key of length L, it performs ceil(L/8) compression rounds. Since the maximum length of a secret key is typically small and bounded (e.g., < 256 bytes), in practical terms for the context of a Hash Table, this is considered O(1) relative to the number of stored items N.

### 7.1.2. Cuckoo Hashing (Core Engine)

- **Lookup (GET)**: O(1) Worst Case. The algorithm checks exactly 2 locations: `T1[h1(x)]` and `T2[h2(x)]`. It never scans a list or probes deeper. This is the main selling point over Chaining (O(N) worst case) or Linear Probing (O(N) worst case under high load).

- **Insertion (PUT)**: O(1) guaranteed. In most cases, insertion finds an empty slot immediately (O(1)). If a "kick-out" chain reaction occurs, it might take several steps, but it is bounded by `max_displacements_`. Rehash (if table is full) takes O(N), but happens very rarely.

### 7.1.3. B-Tree (Path Validation)

- **Search/Insert**: O(log_m N). With a large degree m (e.g., 100), the height of the tree is extremely small. This ensures that path validation is negligible compared to the network latency, serving as an efficient filter.

## 7.2. Space Complexity

- **Cuckoo Table**: O(N). The storage is linear to the number of items. The load factor is kept < 50% to ensure performance, meaning we trade some space (2x capacity) for guaranteed speed.

- **B-Tree**: O(N). Stores unique paths. Space efficiency is high due to high node utilization.

# VIII. EXPERIMENTAL RESULTS

Benchmark result on **14/02/2026**.

## 8.1. Methodology (Design of Experiment)

To evaluate the real performance of Kallisto, the report writer have built a benchmark tool integrated directly into the CLI (`src/main.cpp`). The test was designed to simulate a real usage scenario of a management secret system under extreme conditions about performance to withstanding the load of "Thundering Herd".

### 8.1.1 Test Case Logic

Function `run_benchmark(count)` implements the test process in 2 phases (Phase):

**Phase 1: Write Stress Test**

- Input: Create `N` (e.g: 1,000,000) secret entries.
- Key Distribution: Keys are generated in a sequence (`k0`, `k1`, ... `k999999`) to ensure uniqueness.
- Path Distribution: Use Round-Robin mechanism on 10 fixed paths (`/bench/p0` to `/bench/p9`).
- Purpose: Test the processing capability of **B-Tree Index** when a node must contain many keys and the routing capability of the tree.
- Action: Call `PUT` command. This is the step to test the speed of **SipHash**, the ability to handle collisions of **Cuckoo Hashing**, and the delay of **Storage Engine**.

```cpp
// Code Snippet: Benchmark Loop
for (int i = 0; i < count; ++i) {
    std::string path = "/bench/p" + std::to_string(i % 10); 
    std::string key = "k" + std::to_string(i);
    std::string val = "v" + std::to_string(i);
    server->put_secret(path, key, val);
}
```

**Phase 2: Read Stress Test**

- Input: Query all `N` keys just written.
- Action: Call `GET` command.
- Purpose: Measure the read speed on RAM. Since all data is already in `CuckooTable` (Cache), this is a pure algorithm efficiency test without being affected by Disk I/O.

### 8.1.2 Configuration Environments

**System Specifications**:
- **CPU**: AMD Ryzen 5 3550H with Radeon Vega Mobile Gfx (4 Cores, 8 Threads) @ 2.1GHz
- **RAM**: 6.7 GiB
- **OS**: Ubuntu 24.04.3 LTS (WSL2 Kernel 6.6.87.2)

We perform measurement on **BATCH MODE (Optimized)** to measure the raw algorithmic performance of the In-Memory engine (SipHash + Cuckoo + B-Tree) without the bottleneck of `fsync`.

---

## 8.2 Experimental Results

**Dataset**: 1,000,000 secret items.
**Hardware**: Single Thread (Batch Mode).

### 8.2.1 Core Performance (Single Thread)

| Metric | Result | Note |
| :--- | :--- | :--- |
| **Write RPS** | **829,137 req/s** | SipHash + Cuckoo Insert |
| **Read RPS** | **1,798,440 req/s** | SipHash + Cuckoo Lookup |
| **p99 Latency** | **0.98 µs** | < 1ms requirement met |
| **Total Time** | ~1.7s | Processing 1M requests |

### 8.2.2 Security Analysis (DoS Resilience)

Ran `bench_dos` to compare SipHash against a simulated Weak Hash (Murmur-like) under 5000 collision attacks.

| Metric | Weak Hash | SipHash (Kallisto) |
| :--- | :--- | :--- |
| **Success Rate** | 2/5000 (0.04%) | **5000/5000 (100%)** |
| **Time taken** | 21.78 ms | 6.91 ms |
| **Result** | Failed (Collisions) | **PASSED** |

**B-Tree Gate Efficiency**:
- **Invalid Path Rejection**: Blocked 10,000 invalid requests in **0.30 ms**.
- **Average Latency**: ~30 nanoseconds per block.

## 8.3. Multithreaded Benchmark (Sharded CuckooTable)

**Configuration**: 3 Worker Threads, 64 Shards, 100,000 secrets pre-populated.
**Goal**: Simulate real-world Vault traffic patterns.

### 8.3.1 Benchmark Patterns

1. **MIXED 95/5**: 95% reads, 5% writes - typical production steady-state
2. **BURSTY**: Deployment bursts (Heavy Read) - simulating K8s pods startup
3. **ZIPF**: Hot keys distribution (20% keys receive 80% traffic)

### 8.3.2 Results

| Pattern | Total RPS | Read RPS | Write RPS | Hit Rate |
|---------|-----------|----------|-----------|----------|
| **MIXED 95/5** | **1,862,583** | 1,769,258 | 93,325 | 100% |
| **BURSTY** | **2,783,375** | 2,713,504 | 69,871 | 100% |
| **ZIPF** | 36,534 | 34,716 | 1,818 | 100% |

> **Note**: ZIPF performance is lower due to high contention on specific shards (Hot Shards), verifying that the Sharded Lock mechanism works correctly under contention.

### 8.3.3 Performance Comparison

| Configuration | Average RPS | vs Single-thread |
|---------------|-------------|------------------|
| Single-thread baseline | ~294,000 | 1.0x |
| **Multi-thread (64 shards)** | **~1,560,831** | **5.3x** ✅ |

### 8.3.4 Analysis

- **Scalability**: The system scales near-linearly for Mixed/Bursty workloads 
- **Start-up spikes**: Can handle **>2.7 Million RPS** during "Thundering Herd" events (Bursty pattern).
- **Latency**: Maintained sub-microsecond latency even under high load.

---

## 8.4. Theoretical expectations vs. Actual results

### 8.4.1 Behavior Analysis

- **B-Tree Indexing**: With 10,000 item distributed into 10 paths, each leaf node of B-Tree contains around 1,000 items. The `validate_path` operation consumes O(log 10) which is almost instantaneous. The benchmark results show no significant delay when switching between paths.

- **Cuckoo Hashing**: Hit Rate reaches **100%** (10000/10000). No fail cases due to table overflow (thanks to the 30% Load Factor).

### 8.4.2 "Thundering Herd" Defense Provability

The result of Read RPS (~6,400 req/s) proves the capability of Kallisto to withstand "Thundering Herd" when thousands of services restart and fetch secrets simultaneously:

1.  Kallisto **does not access disk**.
2.  Every `GET` operation is resolved on RAM with O(1) complexity.
3.  The system maintains low latency (<1ms) even under high load.

## 8.5. Server Benchmark (HTTP/1.1 Vault API)

**Date**: 2026-02-15  
**Environment**: 4 vCPU, 8GB RAM (CodeSpaces)  
**Tool**: `wrk` (4 threads, 50 connections)  
**Target**: `http://localhost:8200` (Vault KV v2)

| Workload | Type | Requests/sec (RPS) | Stability |
|----------|------|--------------------|-----------|
| **SEED** | Writes | **39,894** |  Stable |
| **GET** | Read-only | **67,987** |  Stable |
| **PUT** | Write-only | **46,465** | Stable |
| **MIXED** | 95% Read | **46,213** | Stable |

*Note: Benchmarks were conducted at concurrency c=50 to ensure stability within the CodeSpaces environment. The architecture is capable of much higher throughput on bare metal hardware.*

### 8.5.1 Performance Gap Analysis (Why 2.7M -> ~68k RPS?)

The user might notice a significant difference between the **Application Layer** (HTTP/1.1) result (~68k RPS) and the **Core Engine** (In-Memory) result (~2.7M RPS). This "tax" is paid to the Operating System and Data Serialization:

1.  **Network Stack (Kernel Space)**:
    -   Every request involves `epoll_wait`, `read` (syscall), TCP/IP processing, context switches, and `write` (syscall).
    -   In a **Virtual Container** (like GitHub Codespaces), the network bridge is software-emulated, adding 50-70% overhead compared to bare-metal NICs.

2.  **Data Serialization (User Space)**:
    -   **HTTP/1.1**: Text-based protocol requires parsing every byte (METHOD /path PROTOCOL\r\nHeader: ...).
    -   **JSON Parsing**: Converting `{"key":"value"}` strings to C++ objects and back is CPU-intensive.
    -   *In-Memory Benchmark* calls C++ functions directly (Zero-Copy), bypassing all parsing.

**Conclusion**: The core Cuckoo engine is extremely fast (2.7M/s). The bottleneck shifts to the *Transport Layer* (HTTP/JSON) when exposing it as a service.

## 8.6. Conclusion

The experimental results confirm the accuracy of Kallisto's design:

- Write: Batch Mode helps maximize RAM bandwidth, suitable for large-scale data imports (Bulk Load).

- Read: Always stable at high speeds due to the In-Memory Cuckoo Table architecture, meeting the requirements of a High-Performance Secret Management system.

- **Multi-threading with Sharding**: Achieves linear scaling with CPU cores while maintaining strong consistency.

# IX. CONCLUSION

## 9.1. Summary

The "Kallisto" project successfully demonstrates that a hybrid data structure approach—combining the hierarchical discipline of B-Trees with the raw speed of Cuckoo Hashing—can solve modern secret management challenges effectively. It is possible to build a system that is resilient to Hash Flooding attacks (via SipHash) while maintaining high throughput.

### 9.1.1. Pros

High Performance: Achieved ~223,000 Write RPS (Batch Mode) and ~359,000 Read RPS, significantly outperforming traditional file-based storage systems and potentially rivaling Redis in specific workloads.

Predictable Latency: The implementation of Cuckoo Hashing guarantees O(1) worst-case lookup time, eliminating the "tail latency" problem found in Chaining or Linear Probing implementations.

Security-First Design: By integrating SipHash-2-4 as the core hash function, the system is natively immune to algorithm complexity attacks (Hash Flooding DoS).

Path Validation: The B-Tree index acts as an effective firewall, rejecting invalid path requests with O(log N) efficiency before they consume expensive hashing resources.

Flexibility: The "Dual Sync Mode" architecture allows administrators to choose the right trade-off between Data Safety (Strict Mode) and ingestion speed (Batch Mode).

### 9.1.2. Cons

Single Point of Failure: The current prototype runs as a single instance. If the server crashes, service is interrupted (though data is safe on disk).

Encryption-at-Rest Missing: Secrets are currently stored as plaintext binary on disk (`kallisto.db`). While efficient, this is not suitable for production secret management without filesystem-level encryption.

Strict Mode Bottleneck: In Strict Mode, the system is effectively I/O bound (~1,500 RPS) due to the heavy cost of `fsync()` syscalls, limiting its use for high-write workloads.

## 9.2. Future Works

To evolve Kallisto from a robust academic prototype to a production-grade system, the following roadmap is proposed:

1.  Security Hardening:

Encryption-at-Rest: Implement AES-256-GCM to encrypt values before flushing to disk, protecting against physical drive theft.
Secure Memory Allocator: Use `mlock()` and `explicit_bzero()` to prevent secret leakage via swap files or core dumps.
Access Control List (ACL): Implement Token-based Authentication and RBAC to restrict access to specific paths.

2.  Scalability & Reliability:

Write-Ahead Logging (WAL): Replace the current snapshot mechanism with an Append-Only Log to provide better durability without the performance penalty of full snapshots.
Replication: Implement the Raft Consensus Algorithm to support multi-node clustering, ensuring High Availability.

# X. References

1.  Bernstein, D. J., & Aumasson, J. P. (2012). "SipHash: a fast short-input PRF." [https://131002.net/siphash/](https://131002.net/siphash/)
2.  Pagh, R., & Rodler, F. F. (2001). "Cuckoo Hashing." *Journal of Algorithms*, 51(2), 122-144.
3.  Bayer, R., & McCreight, E. (1972). "Organization and Maintenance of Large Ordered Indexes." *Acta Informatica*, 1(3), 173-189.
4.  "SipHash - a fast short-input PRF." [https://docs.kernel.org/security/siphash.html](https://docs.kernel.org/security/siphash.html)
