// mixed_bench.js: MIXED workload benchmark (95% READ, 5% WRITE)
// Simulates production traffic pattern for Vault-like secret management
//
// Usage: k6 run --vus 100 --duration 10s benchmarks/server/workloads/mixed_bench.js

import http from 'k6/http';
import { check } from 'k6';
import { Counter } from 'k6/metrics';
import { SharedArray } from 'k6/data';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8200';

// Custom counters to track read/write ratio in the summary output
const readRequests = new Counter('read_requests');
const writeRequests = new Counter('write_requests');

// Pre-generate 256000 paths for reads and write payloads
const paths = new SharedArray('mixed_paths', function () {
	const items = [];
	for (let i = 0; i < 256000; i++) {
		items.push({
			path: `/v1/secret/data/bench/s${i}`,
			writeBody: JSON.stringify({ data: { value: `updated-${i}` } }),
		});
	}
	return items;
});

export const options = {
	thresholds: {
		http_req_duration: ['p(99)<80'],
		http_req_failed: ['rate<0.01'],
	},
};

export default function () {
	const idx = __ITER % paths.length;
	const item = paths[idx];

	// 5% writes (same ratio as the original wrk_mixed.lua)
	if (Math.random() < 0.05) {
		writeRequests.add(1);
		const res = http.post(`${BASE_URL}${item.path}`, item.writeBody, {
			headers: { 'Content-Type': 'application/json' },
			tags: { type: 'write' },
		});
		check(res, {
			'write status ok': (r) => r.status === 200 || r.status === 204,
		});
	} else {
		readRequests.add(1);
		const res = http.get(`${BASE_URL}${item.path}`, {
			tags: { type: 'read' },
		});
		check(res, {
			'read status is 200': (r) => r.status === 200,
		});
	}
}
