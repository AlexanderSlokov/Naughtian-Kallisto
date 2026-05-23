-- wrk_seed.lua: Seed data for GET benchmark
-- Seeds keys bench/s0..s999 so wrk_get.lua can read them
--
-- Usage: wrk -t2 -c10 -d3s -s benchmarks/server/workloads/wrk_seed.lua http://localhost:8200

counter = -1

wrk.method = "POST"
wrk.headers["Content-Type"] = "application/json"

request = function()
    counter = counter + 1
    local id = counter % 1000
    local path = "/v1/secret/data/bench/s" .. id
    local body = '{"data":{"key":"seed-value-' .. id .. '","index":' .. id .. '}}'
    return wrk.format("POST", path, nil, body)
end

done = function(summary, latency, requests)
    io.write(string.format("[SEED] %d requests in %.2fs (%.0f req/s)\n",
        summary.requests,
        summary.duration / 1000000,
        summary.requests / (summary.duration / 1000000)))
end
