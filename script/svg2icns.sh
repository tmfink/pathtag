#!/usr/bin/env bash

set -eu

# Check for required arguments
if [ "$#" -lt 1 ] || [ $# -gt 2 ]; then
    echo "Usage: $0 INPUT_SVG [OUTPUT_ICNS]"
    exit 1
fi

RSVG_CONVERT="rsvg-convert"

SVG_INPUT="$1"
shift

if [ $# -gt 0 ]; then
    OUTPUT_ICNS="$1"
else
    OUTPUT_ICNS="${SVG_INPUT%.*}.icns"
fi

echo "Writing output ICNS to: ${OUTPUT_ICNS}"

if ! type "${RSVG_CONVERT}" >/dev/null; then
    echo "Error: '${RSVG_CONVERT}' is not an executable file or does not exist."
    exit 1
fi

if [ ! -f "${SVG_INPUT}" ]; then
    echo "Error: SVG file '${SVG_INPUT}' not found."
    exit 1
fi

TMP_DIR="$(mktemp -d -t "svg2icns.XXXXXX")"

ICONSET_DIR=${TMP_DIR}/tmp.iconset
mkdir "${ICONSET_DIR}"

cleanup() {
    if [ -n "${SKIP_CLEANUP:-}" ]; then
        echo "Skipping cleanup"
        return
    fi
    echo "Cleaning up temporary files..."
    rm -rf "${TMP_DIR}"
}

# Register the trap to run cleanup on script exit, interruption (Ctrl+C), or termination
trap cleanup EXIT INT TERM

# Define the standard dimensions required by macOS iconutil
# Format: size_name:width_and_height_for_1x:width_and_height_for_2x
SIZES=(
    "16x16:16:32"
    "32x32:32:64"
    "128x128:128:256"
    "256x256:256:512"
    "512x512:512:1024"
)

for item in "${SIZES[@]}"; do
    IFS=":" read -r name size1x size2x <<<"${item}"

    # Generate standard resolution (1x)
    "${RSVG_CONVERT}" -w "${size1x}" -h "${size1x}" "${SVG_INPUT}" -o "${ICONSET_DIR}/icon_${name}.png"

    # Generate high resolution (2x Retina)
    "${RSVG_CONVERT}" -w "${size2x}" -h "${size2x}" "${SVG_INPUT}" -o "${ICONSET_DIR}/icon_${name}@2x.png"
done

echo "Compiling to ICNS..."
iconutil -c icns "${ICONSET_DIR}" -o "./${OUTPUT_ICNS}"

echo "Success! Created: ${OUTPUT_ICNS}"
