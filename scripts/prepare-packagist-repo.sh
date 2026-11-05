#!/usr/bin/env bash
# Sync packages/octane-driver → octane-driver-rearm/ for GitHub k1roshio/octane-driver-rearm
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${ROOT}/packages/octane-driver"
DST="${ROOT}/octane-driver-rearm"

# Prefer renamed folder; fall back to legacy path during transition
if [[ ! -d "${DST}" && -d "${ROOT}/octane-driver-divine" ]]; then
  mv "${ROOT}/octane-driver-divine" "${DST}"
fi

if [[ ! -f "${SRC}/composer.json" ]]; then
  echo "missing ${SRC}/composer.json" >&2
  exit 1
fi

mkdir -p "${DST}"
if command -v rsync >/dev/null 2>&1; then
  rsync -a --delete \
    --exclude '.git' \
    --exclude 'vendor' \
    --exclude 'README.md' \
    "${SRC}/" "${DST}/"
else
  # keep DST README if present
  tmp_readme=""
  if [[ -f "${DST}/README.md" ]]; then
    tmp_readme="$(mktemp)"
    cp "${DST}/README.md" "${tmp_readme}"
  fi
  rm -rf "${DST:?}/"*
  # keep .git
  cp -a "${SRC}/." "${DST}/"
  rm -rf "${DST}/vendor"
  if [[ -n "${tmp_readme}" ]]; then
    mv "${tmp_readme}" "${DST}/README.md"
  fi
fi

cp -f "${ROOT}/LICENSE" "${DST}/LICENSE"

# Standalone README is maintained in octane-driver-rearm/README.md (not overwritten by rsync exclude).

if [[ ! -f "${DST}/.gitignore" ]]; then
  cat >"${DST}/.gitignore" <<'EOF'
/vendor/
composer.lock
.phpunit.result.cache
.idea/
.vscode/
.DS_Store
*.log
EOF
fi

# Ensure Packagist/GitHub metadata on composer.json in the export folder
php -r '
$p = $argv[1];
$j = json_decode(file_get_contents($p), true);
$j["homepage"] = "https://github.com/k1roshio/octane-driver-rearm";
$j["support"] = [
  "issues" => "https://github.com/k1roshio/octane-driver-rearm/issues",
  "source" => "https://github.com/k1roshio/octane-driver-rearm",
];
if (!empty($j["authors"][0]) && empty($j["authors"][0]["homepage"])) {
  $j["authors"][0]["homepage"] = "https://github.com/k1roshio";
}
file_put_contents($p, json_encode($j, JSON_PRETTY_PRINT|JSON_UNESCAPED_SLASHES)."\n");
' "${DST}/composer.json"

echo "Prepared ${DST}"
echo "Next:"
echo "  cd octane-driver-rearm"
echo "  git init && git add . && git commit -m 'Initial k1roshio/octane-driver-rearm'"
echo "  git remote add origin https://github.com/k1roshio/octane-driver-rearm.git"
echo "  git push -u origin main && git tag v0.1.0 && git push origin v0.1.0"
