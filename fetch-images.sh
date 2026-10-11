#!/bin/sh
# fetch-images.sh — Download and verify the B206mini firmware/FPGA images.
# Parses UHD's own images/manifest.txt for the two B2xx packages this device
# needs, fetches them from the Ettus binary cache, verifies each SHA256
# against the manifest, and unpacks them flat into $SDRPREFIX/share/uhd/images.
set -eu

MANIFEST=${1:-$HOME/.sdr/uhd/images/manifest.txt}   # manifest path (arg 1 overrides)
BASE=https://files.ettus.com/binaries/cache        # Ettus binary cache root
IMAGES=${SDRPREFIX:?export SDRPREFIX first}/share/uhd/images  # where libuhd looks

mkdir -p "$IMAGES"
cd "$IMAGES"

# Manifest columns: target-name  version  cache-path  sha256.
# The b206mini FPGA target points at the b205mini zip, so matching
# common_fw + b205mini_fpga covers everything the B206mini needs.
awk '$1 ~ /^b2xx_(common_fw|b205mini_fpga)_default$/ {print $3, $4}' "$MANIFEST" |
while read -r path sha; do
    file=$(basename "$path")                       # zip filename from cache path
    echo "==> $file"
    curl -fLO "$BASE/$path"                        # -f: fail loudly on HTTP errors
    echo "$sha  $file" | shasum -a 256 -c -        # verify against manifest hash
    if command -v unzip >/dev/null 2>&1; then
        unzip -jo "$file"                          # -j flatten, -o overwrite
    else                                           # a minimal Debian has no unzip
        python3 -c 'import os, sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    for m in z.infolist():
        if m.is_dir():
            continue
        with z.open(m) as src, open(os.path.basename(m.filename), "wb") as dst:
            dst.write(src.read())
        print("  inflating:", os.path.basename(m.filename))' "$file"
    fi
done

echo "==> images installed:" && ls "$IMAGES"
