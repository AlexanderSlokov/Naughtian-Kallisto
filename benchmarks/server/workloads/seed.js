// seed.js: Seed data for GET/MIXED benchmarks
// Seeds keys bench/s0..s999 so get_bench.js and mixed_bench.js can read them.
//
// Usage: k6 run --vus 10 --duration 3s benchmarks/server/workloads/seed.js

import http from 'k6/http';
import { SharedArray } from 'k6/data';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8200';

// Pre-generate 256000 seed payloads to avoid per-request string allocation
const seeds = new SharedArray('seed_payloads', function () {
  const items = [];
  for (let i = 0; i < 256000; i++) {
    items.push({
      path: `/v1/secret/data/bench/s${i}`,
      body: JSON.stringify({ data: { key: `seed-value-${i}`, index: i } }),
    });
  }
  return items;
});

export const options = {
  // Thresholds intentionally relaxed — seeding is a setup step, not a benchmark
  thresholds: {
    http_req_failed: ['rate<0.05'],
  },
};

export default function () {
  const idx = __ITER % seeds.length;
  const seed = seeds[idx];

  http.post(`${BASE_URL}${seed.path}`, seed.body, {
    headers: { 'Content-Type': 'application/json' },
    tags: { type: 'seed' },
  });
}
