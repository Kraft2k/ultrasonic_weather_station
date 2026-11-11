#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
image=${1:?Pass the application ELF path}
shift
# Never silently choose a bootloader from another build profile/configuration.
profile_dir=$(dirname -- "$image")
shopt -s nullglob
bootloaders=("$profile_dir"/build/esp-idf-sys-*/out/build/bootloader/bootloader.bin)
if (( ${#bootloaders[@]} != 1 )); then
    echo "Expected one ESP-IDF bootloader under $profile_dir/build; found ${#bootloaders[@]}" >&2
    exit 1
fi
exec espflash flash --before "${ESPFLASH_BEFORE:-default-reset}" \
    --bootloader "${bootloaders[0]}" --flash-mode dio --flash-freq 80mhz \
    --monitor "$@" "$image"
