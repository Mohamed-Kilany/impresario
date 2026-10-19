#!/usr/bin/env bash
#
# Regenerates the committed Swift protobuf/gRPC sources of the Apple agent
# (TASK.md section 12.1: generated code is committed and regenerable).
#
# Tools (exact versions per docs/DECISIONS.md I-83):
#   protoc               >= 3.20  (protoc-29.3 was used)
#   protoc-gen-swift     swift-protobuf 1.39.0
#   protoc-gen-grpc-swift grpc-swift 1.27.6
#
# Build the plugins from the pinned checkouts, then run this script:
#   (cd <swift-protobuf>  && swift build -c release --product protoc-gen-swift)
#   (cd <grpc-swift>      && swift build -c release --product protoc-gen-grpc-swift)
#   PATH="<swift-protobuf>/.build/release:<grpc-swift>/.build/release:$PATH" \
#     scripts/gen-swift.sh
#
# Override the tool locations with PROTOC / PROTOC_GEN_SWIFT /
# PROTOC_GEN_GRPC_SWIFT when they are not on PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROTO_DIR="$ROOT/proto"
OUT_DIR="$ROOT/agents/apple/Sources/ImpresarioWire"

PROTOC="${PROTOC:-protoc}"
PROTOC_GEN_SWIFT="${PROTOC_GEN_SWIFT:-protoc-gen-swift}"
PROTOC_GEN_GRPC_SWIFT="${PROTOC_GEN_GRPC_SWIFT:-protoc-gen-grpc-swift}"

mkdir -p "$OUT_DIR"
cd "$PROTO_DIR"

"$PROTOC" \
    --plugin=protoc-gen-swift="$PROTOC_GEN_SWIFT" \
    --plugin=protoc-gen-grpc-swift="$PROTOC_GEN_GRPC_SWIFT" \
    --swift_out="$OUT_DIR" \
    --swift_opt=Visibility=Public \
    --grpc-swift_out=Visibility=Public:"$OUT_DIR" \
    -I . \
    impresario/v1/impresario.proto

# protoc preserves the proto's directory layout; keep the module flat.
if [ -d "$OUT_DIR/impresario" ]; then
    find "$OUT_DIR/impresario" -name '*.swift' -exec mv {} "$OUT_DIR/" \;
    rm -rf "$OUT_DIR/impresario"
fi

echo "generated into $OUT_DIR:"
ls -l "$OUT_DIR"
