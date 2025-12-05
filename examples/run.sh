#!/bin/bash

set -e

cd "$(dirname "$0")/.."

cleanup() {
    if [ -n "$SERVER_PID" ]; then
        kill $SERVER_PID 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo "Building examples..."
cargo build --example grpc_server --example grpc_client --quiet

echo "Starting gRPC server..."
cargo run --example grpc_server --quiet &
SERVER_PID=$!
sleep 2

echo ""
echo "=== Query 1: Get user with their posts ==="
cargo run --example grpc_client --quiet -- '{ user(id: 1) { id name email posts { title } } }'

echo ""
echo "=== Query 2: List all users ==="
cargo run --example grpc_client --quiet -- '{ users { id name email } }'

echo ""
echo "=== Query 3: Get posts with authors and comments ==="
cargo run --example grpc_client --quiet -- '{ posts { title author { name } comments { text author { name } } } }'

echo ""
echo "=== Query 4: Get post with likes ==="
cargo run --example grpc_client --quiet -- '{ post(id: 1) { title likeCount likes { user { name } createdAt } } }'

echo ""
echo "All tests passed!"
