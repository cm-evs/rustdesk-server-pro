#!/bin/sh
# Build the rustdesk-server-pro Docker image locally from a published
# linux-x86_64-glibc release package (or an unpacked directory).
#
# Usage:
#   docker/build-local.sh [--classic] [--tag TAG] [PACKAGE_OR_DIR]
#
#   PACKAGE_OR_DIR  Release zip (rustdesk-server-pro-linux-x86_64-glibc-v*.zip),
#                   .tar.gz, or a directory containing hbbs and hbbr.
#                   Default: newest rustdesk-server-pro-linux-x86_64-glibc-v*.zip
#                   in ./dist-pro.
#   --classic       Build the classic image (docker-classic/) instead of the
#                   s6 image (docker/).
#   --tag TAG       Image tag. Default: ghcr.io/wirh/rustdesk-server-pro:local
#                   (classic: ghcr.io/wirh/rustdesk-server-pro-classic:local).
#
# Examples:
#   docker/build-local.sh
#   docker/build-local.sh ./dist-pro/rustdesk-server-pro-linux-x86_64-glibc-v45.zip
#   docker/build-local.sh --classic --tag myreg/pro-classic:local ./unpacked-dir
#
# The script copies hbbs/hbbr (and rustdesk-utils when available) into the
# build context, runs docker build, then removes the staged binaries again
# (files that were already there are restored). Requires: docker, and
# unzip (for .zip) or tar (for .tar.gz).
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(dirname "$SCRIPT_DIR")

CLASSIC=0
TAG=""
SRC=""

usage() {
    sed -n '2,25p' "$0" | sed 's/^# \?//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --classic) CLASSIC=1; shift ;;
        --tag) [ $# -ge 2 ] || { echo "error: --tag needs a value" >&2; exit 2; }; TAG=$2; shift 2 ;;
        --tag=*) TAG=${1#--tag=}; shift ;;
        -h|--help) usage; exit 0 ;;
        --) shift; break ;;
        -*) echo "error: unknown option: $1" >&2; usage >&2; exit 2 ;;
        *) [ -z "$SRC" ] || { echo "error: only one PACKAGE_OR_DIR allowed" >&2; exit 2; }; SRC=$1; shift ;;
    esac
done
[ $# -eq 0 ] || { echo "error: unexpected argument: $1" >&2; exit 2; }

command -v docker >/dev/null 2>&1 || { echo "error: docker not found in PATH" >&2; exit 1; }

if [ "$CLASSIC" -eq 1 ]; then
    IMAGE=${TAG:-ghcr.io/wirh/rustdesk-server-pro-classic:local}
    CONTEXT="$REPO_ROOT/docker-classic"
    STAGE_DIR="$REPO_ROOT/docker-classic"
else
    IMAGE=${TAG:-ghcr.io/wirh/rustdesk-server-pro:local}
    CONTEXT="$REPO_ROOT/docker"
    STAGE_DIR="$REPO_ROOT/docker/rootfs/usr/bin"
fi

# Default source: newest linux glibc release package.
if [ -z "$SRC" ]; then
    for d in "$REPO_ROOT/dist-pro"; do
        SRC=$(ls -t "$d"/rustdesk-server-pro-linux-x86_64-glibc-v*.zip 2>/dev/null | head -1 || true)
        [ -n "$SRC" ] && break
    done
    [ -n "$SRC" ] || { echo "error: no rustdesk-server-pro-linux-x86_64-glibc-v*.zip found in ./dist-pro; pass a package path or directory" >&2; exit 1; }
    echo "using default package: $SRC"
fi
[ -e "$SRC" ] || { echo "error: no such file or directory: $SRC" >&2; exit 1; }

BACKUP_DIR=$(mktemp -d)
EXTRACT_DIR=""
STAGED=""
RESTORE=""

cleanup() {
    for f in $STAGED; do rm -f "$f"; done
    for f in $RESTORE; do mv -f "$BACKUP_DIR/$(basename "$f")" "$f" 2>/dev/null || true; done
    [ -n "$EXTRACT_DIR" ] && rm -rf "$EXTRACT_DIR"
    rm -rf "$BACKUP_DIR"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

stage() {
    # stage <src-file> <dest-file>: move any existing dest file aside first.
    if [ -e "$2" ]; then
        mv "$2" "$BACKUP_DIR/$(basename "$2")"
        RESTORE="$RESTORE $2"
    fi
    cp "$1" "$2"
    chmod +x "$2"
    STAGED="$STAGED $2"
}

# Resolve the directory holding the binaries.
case "$SRC" in
    *.zip)
        command -v unzip >/dev/null 2>&1 || { echo "error: unzip not found in PATH" >&2; exit 1; }
        EXTRACT_DIR=$(mktemp -d)
        unzip -q "$SRC" -d "$EXTRACT_DIR"
        BIN_SRC="$EXTRACT_DIR"
        ;;
    *.tar.gz|*.tgz)
        EXTRACT_DIR=$(mktemp -d)
        tar -xzf "$SRC" -C "$EXTRACT_DIR"
        BIN_SRC="$EXTRACT_DIR"
        ;;
    *)
        [ -d "$SRC" ] || { echo "error: not a directory, .zip or .tar.gz: $SRC" >&2; exit 1; }
        BIN_SRC="$SRC"
        ;;
esac

HBBS_BIN=$(find "$BIN_SRC" -name hbbs -type f 2>/dev/null | head -1 || true)
HBBR_BIN=$(find "$BIN_SRC" -name hbbr -type f 2>/dev/null | head -1 || true)
[ -n "$HBBS_BIN" ] || { echo "error: hbbs not found in $SRC" >&2; exit 1; }
[ -n "$HBBR_BIN" ] || { echo "error: hbbr not found in $SRC" >&2; exit 1; }
UTILS_BIN=$(find "$BIN_SRC" -name rustdesk-utils -type f 2>/dev/null | head -1 || true)
if [ -z "$UTILS_BIN" ] && [ -x "$REPO_ROOT/target/release/rustdesk-utils" ]; then
    UTILS_BIN="$REPO_ROOT/target/release/rustdesk-utils"
fi

echo "staging binaries from: $(dirname "$HBBS_BIN")"
stage "$HBBS_BIN" "$STAGE_DIR/hbbs"
stage "$HBBR_BIN" "$STAGE_DIR/hbbr"
if [ "$CLASSIC" -eq 0 ]; then
    if [ -n "$UTILS_BIN" ]; then
        stage "$UTILS_BIN" "$STAGE_DIR/rustdesk-utils"
    else
        echo "warning: rustdesk-utils not found; key validation via KEY_PUB/KEY_PRIV or docker secrets will fail in this image (keys generated by hbbs itself still work)" >&2
    fi
fi

if [ "$CLASSIC" -eq 1 ]; then
    docker build --platform linux/amd64 -t "$IMAGE" "$CONTEXT"
else
    docker build --platform linux/amd64 --build-arg S6_ARCH=x86_64 -t "$IMAGE" "$CONTEXT"
fi

echo "built $IMAGE"
if [ "$CLASSIC" -eq 1 ]; then
    echo "run it via docker-compose.yml, or: docker run -d -p 21114:21114 -p 21116:21116 $IMAGE hbbs -r <relay-host>:21117"
else
    echo "run it with, e.g.: docker run -d --name rustdesk-pro -p 21114:21114 -p 21115:21115 -p 21116:21116 -p 21116:21116/udp -p 21117:21117 -p 21118:21118 -p 21119:21119 -v rustdesk-pro-data:/data -e RELAY=<relay-host> $IMAGE"
fi
