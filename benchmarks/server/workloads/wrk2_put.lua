-- wrk2_put.lua: Minimal POST script for wrk2 release benchmarks
-- Used for both seeding and PUT throughput measurement
--
-- Usage: wrk2 -t2 -c200 -d10s -R 100000 -s wrk2_put.lua http://localhost:8200

counter = -1

wrk.method = "POST"
wrk.headers["Content-Type"] = "application/json"

request = function()
    counter = counter + 1
    local id = counter % 256000
    local path = "/v1/secret/data/bench/s" .. id
    local body = '{"data":{"value":"bench-' .. id .. '"}}'
    return wrk.format("POST", path, nil, body)
end
