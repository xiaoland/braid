#!/usr/bin/env bash
set -euo pipefail
npm ci --prefix tests/browser --ignore-scripts
export CHROME_BIN=/usr/bin/google-chrome
export BRAID_BINARY="$PWD/target/debug/braid"
cargo build --locked
node --test tests/browser/draft.test.mjs
python3 - <<'PY'
from pathlib import Path
p=Path('src/console/web/console.js')
s=p.read_text()
needle="if(e.body?.requestId&&e.body?.state){renderReceipt(e.body);return;}"
assert s.count(needle)==1
p.write_text(s.replace(needle,"if(e.body?.requestId&&e.body?.state){renderReceipt(e.body);if(current)await select(current.item.number,true);return;}"))
PY
trap 'git restore src/console/web/console.js' EXIT
cargo build --locked
if node --test tests/browser/draft.test.mjs > /tmp/browser-control.log 2>&1; then
  echo 'Reloading a rejected edit escaped detection' >&2
  exit 1
fi
cat /tmp/browser-control.log
grep -F '第一位操作者已保存' /tmp/browser-control.log
grep -F '第二位操作者的草稿' /tmp/browser-control.log
git restore src/console/web/console.js
cargo build --locked
python3 .github/audit-controls/platform-control.py "$BRAID_BINARY" --green
