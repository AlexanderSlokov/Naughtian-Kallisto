warning[no-license-field]: license expression was not specified in manifest for crate 'control_plane = 1.0.0'
 ├ control_plane v1.0.0
   └── (dev) naughtian-kallisto v1.0.0
       └── control_plane v1.0.0 (*)

warning[unlicensed]: a valid license expression could not be retrieved for the crate
  ┌─ path+file:///home/stella/workspace/naughtian-kallisto/components/kallisto_cluster#control_plane@1.0.0-synthesized.toml:2:9
  │
2 │ name = "control_plane"
  │         ━━━━━━━━━━━━━
  │
  ├ control_plane v1.0.0 (*)

error[unlicensed]: control_plane = 1.0.0 is unlicensed
 ├ control_plane v1.0.0 (*)

error[rejected]: failed to satisfy license requirements
   ┌─ /home/stella/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/foca-1.0.0/Cargo.toml:38:12
   │
38 │ license = "MPL-2.0"
   │            ━━━━━━━
   │            │
   │            rejected: license is not explicitly allowed
   │
   ├ MPL-2.0 - Mozilla Public License 2.0:
   ├   - OSI approved
   ├   - FSF Free/Libre
   ├   - Copyleft
   ├ foca v1.0.0
     └── control_plane v1.0.0
         └── (dev) naughtian-kallisto v1.0.0
             └── control_plane v1.0.0 (*)

warning[no-license-field]: license expression was not specified in manifest for crate 'naughtian-kallisto = 1.0.0'
 ├ naughtian-kallisto v1.0.0
   └── control_plane v1.0.0
       └── (dev) naughtian-kallisto v1.0.0 (*)

error[rejected]: failed to satisfy license requirements
  ┌─ path+file:///home/stella/workspace/naughtian-kallisto#1.0.0-synthesized.toml:5:15
  │
5 │ files-expr = "AGPL-3.0-or-later"
  │               ━━━━━━━━━━━━━━━━━
  │               │
  │               license expression retrieved via license files: LICENSE
  │               rejected: license is not explicitly allowed
  │
  ├ AGPL-3.0-or-later - GNU Affero General Public License v3.0 or later:
  ├   - OSI approved
  ├   - FSF Free/Libre
  ├   - Copyleft
  ├ naughtian-kallisto v1.0.0 (*)

warning[license-exception-not-encountered]: license exception was not encountered
    ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:117:15
    │
117 │     { name = "unicode-ident", allow = ["Unicode-DFS-2016"] },
    │               ━━━━━━━━━━━━━ unmatched license exception

warning[license-exception-not-encountered]: license exception was not encountered
    ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:119:15
    │
119 │     { name = "slog-json", allow = ["MPL-2.0"] },
    │               ━━━━━━━━━ unmatched license exception

warning[license-exception-not-encountered]: license exception was not encountered
    ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:120:15
    │
120 │     { name = "smartstring", allow = ["MPL-2.0"] },
    │               ━━━━━━━━━━━ unmatched license exception

warning[license-exception-not-encountered]: license exception was not encountered
    ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:121:15
    │
121 │     { name = "inferno", allow = ["CDDL-1.0"] },
    │               ━━━━━━━ unmatched license exception

warning[license-exception-not-encountered]: license exception was not encountered
    ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:124:15
    │
124 │     { name = "aws-lc-fips-sys", allow = ["OpenSSL"] },
    │               ━━━━━━━━━━━━━━━ unmatched license exception

warning[license-exception-not-encountered]: license exception was not encountered
    ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:126:15
    │
126 │     { name = "webpki-roots", allow = ["CDLA-Permissive-2.0"] },
    │               ━━━━━━━━━━━━ unmatched license exception

warning[license-not-encountered]: license was not encountered
    ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:112:11
    │
112 │ allow = ["0BSD", "Apache-2.0", "BSD-3-Clause", "CC0-1.0", "ISC", "MIT", "Zlib", "Unicode-3.0"]
    │           ━━━━ unmatched license allowance

warning[license-not-encountered]: license was not encountered
    ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:112:49
    │
112 │ allow = ["0BSD", "Apache-2.0", "BSD-3-Clause", "CC0-1.0", "ISC", "MIT", "Zlib", "Unicode-3.0"]
    │                                                 ━━━━━━━ unmatched license allowance

warning[license-not-encountered]: license was not encountered
    ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:112:74
    │
112 │ allow = ["0BSD", "Apache-2.0", "BSD-3-Clause", "CC0-1.0", "ISC", "MIT", "Zlib", "Unicode-3.0"]
    │                                                                          ━━━━ unmatched license allowance

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:10:34
   │
10 │     { name = "md5", wrappers = ["aws", "google-cloud-storage"] },
   │                                  ━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:10:41
   │
10 │     { name = "md5", wrappers = ["aws", "google-cloud-storage"] },
   │                                         ━━━━━━━━━━━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:11:35
   │
11 │     { name = "md-5", wrappers = ["aws-smithy-checksums"]},
   │                                   ━━━━━━━━━━━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:13:35
   │
13 │     { name = "sha1", wrappers = ["aws-smithy-checksums", "stdweb-internal-macros"]},
   │                                   ━━━━━━━━━━━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:13:59
   │
13 │     { name = "sha1", wrappers = ["aws-smithy-checksums", "stdweb-internal-macros"]},
   │                                                           ━━━━━━━━━━━━━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:15:35
   │
15 │     { name = "sha2", wrappers = ["oauth2", "aws-sigv4", "aws-smithy-checksums", "aws-sdk-s3", "google-cloud-storage"] },
   │                                   ━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:15:45
   │
15 │     { name = "sha2", wrappers = ["oauth2", "aws-sigv4", "aws-smithy-checksums", "aws-sdk-s3", "google-cloud-storage"] },
   │                                             ━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:15:58
   │
15 │     { name = "sha2", wrappers = ["oauth2", "aws-sigv4", "aws-smithy-checksums", "aws-sdk-s3", "google-cloud-storage"] },
   │                                                          ━━━━━━━━━━━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:15:82
   │
15 │     { name = "sha2", wrappers = ["oauth2", "aws-sigv4", "aws-smithy-checksums", "aws-sdk-s3", "google-cloud-storage"] },
   │                                                                                  ━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:15:96
   │
15 │     { name = "sha2", wrappers = ["oauth2", "aws-sigv4", "aws-smithy-checksums", "aws-sdk-s3", "google-cloud-storage"] },
   │                                                                                                ━━━━━━━━━━━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:23:39
   │
23 │     { name = "chacha20", wrappers = ["rand"] },
   │                                       ━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:32:35
   │
32 │     { name = "hmac", wrappers = ["aws-sigv4", "aws-sdk-s3"]},
   │                                   ━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:32:48
   │
32 │     { name = "hmac", wrappers = ["aws-sigv4", "aws-sdk-s3"]},
   │                                                ━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:35:37
   │
35 │     { name = "rustls", wrappers = ["gcp_v2", "google-cloud-auth", "reqwest", "tokio-rustls", "hyper-rustls"] },
   │                                     ━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:35:47
   │
35 │     { name = "rustls", wrappers = ["gcp_v2", "google-cloud-auth", "reqwest", "tokio-rustls", "hyper-rustls"] },
   │                                               ━━━━━━━━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:35:68
   │
35 │     { name = "rustls", wrappers = ["gcp_v2", "google-cloud-auth", "reqwest", "tokio-rustls", "hyper-rustls"] },
   │                                                                    ━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:35:79
   │
35 │     { name = "rustls", wrappers = ["gcp_v2", "google-cloud-auth", "reqwest", "tokio-rustls", "hyper-rustls"] },
   │                                                                               ━━━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:35:95
   │
35 │     { name = "rustls", wrappers = ["gcp_v2", "google-cloud-auth", "reqwest", "tokio-rustls", "hyper-rustls"] },
   │                                                                                               ━━━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:36:35
   │
36 │     { name = "ring", wrappers = ["rustls", "rustls-webpki"] },
   │                                   ━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:36:45
   │
36 │     { name = "ring", wrappers = ["rustls", "rustls-webpki"] },
   │                                             ━━━━━━━━━━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:41:37
   │
41 │     { name = "digest", wrappers = ["sha2", "md-5", "sha1", "hmac", "crc-fast"] },
   │                                     ━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:41:45
   │
41 │     { name = "digest", wrappers = ["sha2", "md-5", "sha1", "hmac", "crc-fast"] },
   │                                             ━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:41:53
   │
41 │     { name = "digest", wrappers = ["sha2", "md-5", "sha1", "hmac", "crc-fast"] },
   │                                                     ━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:41:61
   │
41 │     { name = "digest", wrappers = ["sha2", "md-5", "sha1", "hmac", "crc-fast"] },
   │                                                             ━━━━ unmatched wrapper

warning[unused-wrapper]: wrapper for banned crate was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:41:69
   │
41 │     { name = "digest", wrappers = ["sha2", "md-5", "sha1", "hmac", "crc-fast"] },
   │                                                                     ━━━━━━━━ unmatched wrapper

error[unmaintained]: Bincode is unmaintained
   ┌─ /home/stella/workspace/naughtian-kallisto/Cargo.lock:10:1
   │
10 │ bincode 1.3.3 registry+https://github.com/rust-lang/crates.io-index
   │ ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ unmaintained advisory detected
   │
   ├ ID: RUSTSEC-2025-0141
   ├ Advisory: https://rustsec.org/advisories/RUSTSEC-2025-0141
   ├ Due to a doxxing and harassment incident, the bincode team has taken the decision to cease development permanently.
     
     The team considers version 1.3.3 a complete version of bincode that is not in need of any updates.
     
     ## Alternatives to consider
     
     * [wincode](https://crates.io/crates/wincode)
     * [postcard](https://crates.io/crates/postcard)
     * [bitcode](https://crates.io/crates/bitcode)
     * [rkyv](https://crates.io/crates/rkyv)
   ├ Announcement: https://git.sr.ht/~stygianentity/bincode/tree/v3.0/item/README.md
   ├ Solution: No safe upgrade is available!
   ├ bincode v1.3.3
     └── naughtian-kallisto v1.0.0
         └── control_plane v1.0.0
             └── (dev) naughtian-kallisto v1.0.0 (*)

warning[advisory-not-detected]: advisory was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:69:6
   │
69 │     "RUSTSEC-2021-0145",
   │      ━━━━━━━━━━━━━━━━━ no crate matched advisory criteria

warning[advisory-not-detected]: advisory was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:57:6
   │
57 │     "RUSTSEC-2023-0072",
   │      ━━━━━━━━━━━━━━━━━ no crate matched advisory criteria

warning[advisory-not-detected]: advisory was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:62:6
   │
62 │     "RUSTSEC-2024-0357",
   │      ━━━━━━━━━━━━━━━━━ no crate matched advisory criteria

warning[advisory-not-detected]: advisory was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:86:6
   │
86 │     "RUSTSEC-2024-0436",
   │      ━━━━━━━━━━━━━━━━━ no crate matched advisory criteria

warning[advisory-not-detected]: advisory was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:76:6
   │
76 │     "RUSTSEC-2025-0004",
   │      ━━━━━━━━━━━━━━━━━ no crate matched advisory criteria

warning[advisory-not-detected]: advisory was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:83:6
   │
83 │     "RUSTSEC-2025-0022",
   │      ━━━━━━━━━━━━━━━━━ no crate matched advisory criteria

warning[advisory-not-detected]: advisory was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:90:6
   │
90 │     "RUSTSEC-2025-0057",
   │      ━━━━━━━━━━━━━━━━━ no crate matched advisory criteria

warning[advisory-not-detected]: advisory was not encountered
   ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:96:6
   │
96 │     "RUSTSEC-2026-0009",
   │      ━━━━━━━━━━━━━━━━━ no crate matched advisory criteria

warning[advisory-not-detected]: advisory was not encountered
    ┌─ /home/stella/workspace/naughtian-kallisto/deny.toml:101:6
    │
101 │     "RUSTSEC-2026-0097",
    │      ━━━━━━━━━━━━━━━━━ no crate matched advisory criteria

advisories FAILED, bans ok, licenses FAILED, sources ok
