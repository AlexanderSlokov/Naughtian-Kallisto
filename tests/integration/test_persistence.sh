#!/bin/bash
# RocksDB Persistence Test — all output to /workspaces/kallisto/test_log.txt
LOG=tests/test_persistence.log
DB=/tmp/ktest
BIN=./build/kallisto_server

echo "=== RocksDB Persistence Test ===" | tee $LOG
echo "$(date)" | tee -a $LOG

# Cleanup
pkill -f kallisto_server 2>/dev/null
sleep 1
rm -rf $DB

echo "" | tee -a $LOG
echo "--- Step 1: Start server ---" | tee -a $LOG
$BIN --db-path=$DB --workers=1 >> $LOG 2>&1 &
SERVER_PID=$!
echo "Server PID: $SERVER_PID" | tee -a $LOG
sleep 2

echo "" | tee -a $LOG
echo "--- Step 2: PUT ---" | tee -a $LOG
curl -s -X POST http://localhost:8200/v1/secret/data/test/key1 \
  -H "Content-Type: application/json" \
  -d '{"data":{"value":"persistent-value"}}' | tee -a $LOG
echo "" | tee -a $LOG

echo "" | tee -a $LOG
echo "--- Step 3: GET (hot cache) ---" | tee -a $LOG
curl -s http://localhost:8200/v1/secret/data/test/key1 | tee -a $LOG
echo "" | tee -a $LOG

echo "" | tee -a $LOG
echo "--- Step 4: WAL files before shutdown ---" | tee -a $LOG
ls -la $DB/ 2>/dev/null | tee -a $LOG

echo "" | tee -a $LOG
echo "--- Step 5: Graceful SIGTERM ---" | tee -a $LOG
kill $SERVER_PID
wait $SERVER_PID 2>/dev/null
echo "Server stopped (exit: $?)" | tee -a $LOG

echo "" | tee -a $LOG
echo "--- Step 6: WAL files after shutdown ---" | tee -a $LOG
ls -la $DB/ 2>/dev/null | tee -a $LOG

echo "" | tee -a $LOG
echo "--- Step 7: Restart server ---" | tee -a $LOG
$BIN --db-path=$DB --workers=1 >> $LOG 2>&1 &
SERVER_PID=$!
echo "Server PID: $SERVER_PID" | tee -a $LOG
sleep 2

echo "" | tee -a $LOG
echo "--- Step 8: GET after restart (cache-miss → RocksDB fallback) ---" | tee -a $LOG
curl -s http://localhost:8200/v1/secret/data/test/key1 | tee -a $LOG
echo "" | tee -a $LOG

echo "" | tee -a $LOG
echo "--- Step 9: DELETE ---" | tee -a $LOG
curl -s -w "\nHTTP %{http_code}" -X DELETE http://localhost:8200/v1/secret/data/test/key1 | tee -a $LOG
echo "" | tee -a $LOG

echo "" | tee -a $LOG
echo "--- Step 10: GET after DELETE (expect 404) ---" | tee -a $LOG
curl -s http://localhost:8200/v1/secret/data/test/key1 | tee -a $LOG
echo "" | tee -a $LOG

echo "" | tee -a $LOG
echo "--- Step 11: PUT for crash test ---" | tee -a $LOG
curl -s -X POST http://localhost:8200/v1/secret/data/test/crash_key \
  -H "Content-Type: application/json" \
  -d '{"data":{"value":"crash-value"}}' | tee -a $LOG
echo "" | tee -a $LOG

echo "" | tee -a $LOG
echo "--- Step 12: HARD CRASH (kill -9) ---" | tee -a $LOG
# Simulate D1: Server crashes right after responding to PUT
kill -9 $SERVER_PID
wait $SERVER_PID 2>/dev/null
echo "Server crashed (exit: $?)" | tee -a $LOG

echo "" | tee -a $LOG
echo "--- Step 13: Restart server after crash ---" | tee -a $LOG
$BIN --db-path=$DB --workers=1 >> $LOG 2>&1 &
SERVER_PID=$!
echo "Server PID: $SERVER_PID" | tee -a $LOG
sleep 2

echo "" | tee -a $LOG
echo "--- Step 14: GET after crash (D2: WAL fsync verification) ---" | tee -a $LOG
# Verify the key was safely written to the WAL before responding to PUT
curl -s http://localhost:8200/v1/secret/data/test/crash_key | tee -a $LOG
echo "" | tee -a $LOG

echo "" | tee -a $LOG
echo "--- Cleanup ---" | tee -a $LOG
kill $SERVER_PID 2>/dev/null
wait $SERVER_PID 2>/dev/null
rm -rf $DB
echo "DONE" | tee -a $LOG
