// put_bench.js: Pure WRITE benchmark for Kallisto Server
// Tests POST /v1/secret/data/<path> with JSON body
//
// Usage: k6 run --vus 100 --duration 10s benchmarks/server/workloads/put_bench.js

import http from 'k6/http';
import { check } from 'k6';
import { SharedArray } from 'k6/data';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8200';

// Pre-generate 10000 write payloads to match the original wrk_put.lua key space
const payloads = new SharedArray('put_payloads', function () {
  const items = [];
  for (let i = 0; i < 10000; i++) {
    items.push({
      path: `/v1/secret/data/bench/w${i}`,
      body: JSON.stringify({ data: { value: `bench-val-${i}` } }),
    });
  }
  return items;
});

export const options = {
  thresholds: {
    http_req_duration: ['p(99)<100'],
    http_req_failed: ['rate<0.01'],
  },
};

export default function () {
  const idx = __ITER % payloads.length;
  const payload = payloads[idx];

  const res = http.post(`${BASE_URL}${payload.path}`, payload.body, {
    headers: { 'Content-Type': 'application/json' },
    tags: { type: 'write' },
  });

  check(res, {
    'status is 200 or 204': (r) => r.status === 200 || r.status === 204,
  });
}
