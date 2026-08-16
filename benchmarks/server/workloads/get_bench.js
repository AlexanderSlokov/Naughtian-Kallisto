// get_bench.js: Pure READ benchmark for Kallisto Server
// Pre-requisite: Server must be seeded with secrets at /v1/secret/data/bench/s0..s999
//
// Usage: k6 run --vus 100 --duration 10s benchmarks/server/workloads/get_bench.js

import http from 'k6/http';
import { check } from 'k6';
import { SharedArray } from 'k6/data';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8200';

// Pre-generate 256000 paths — SharedArray is read-only and shared across VUs
const paths = new SharedArray('get_paths', function () {
  const items = [];
  for (let i = 0; i < 256000; i++) {
    items.push(`/v1/secret/data/bench/s${i}`);
  }
  return items;
});

export const options = {
  thresholds: {
    http_req_duration: ['p(99)<50'],
    http_req_failed: ['rate<0.01'],
  },
};

export default function () {
  const path = paths[__ITER % paths.length];
  const res = http.get(`${BASE_URL}${path}`, {
    tags: { type: 'read' },
  });

  check(res, {
    'status is 200': (r) => r.status === 200,
    'response has data': (r) => {
      try {
        return r.json().data !== undefined;
      } catch (_) {
        return false;
      }
    },
  });
}
