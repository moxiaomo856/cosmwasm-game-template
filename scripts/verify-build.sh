#!/usr/bin/env bash
# =============================================================================
# cosmwasm-game-template —— 本地构建与结构自检脚本
#
# 用法（WSL）：
#   cd /mnt/g/paxi-tongyong-heyue/dao-tian/cosmwasm-game-template-DRAFT
#   bash scripts/verify-build.sh
#
# 说明：
#   * 链上 wasm 必须用 Rust 1.69.0 产出（1.70+ 的预编译 wasm32 std 带 sign-ext 指令，
#     CosmWasm VM 0.55 不支持，RUSTFLAGS 也关不掉）。宿主机 cargo test 用 1.74.1 即可。
#   * 本脚本只做检查与构建，不做任何链上交易。
# =============================================================================
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
fail=0

ok()   { echo -e "${GREEN}✓${NC} $*"; }
warn() { echo -e "${YELLOW}!${NC} $*"; }
bad()  { echo -e "${RED}✗${NC} $*"; fail=1; }

echo "=============================================================="
echo " 1. 结构自检"
echo "=============================================================="

# 1.1 无感（session）代码必须为零
hits=$(grep -rn "session\|Session" contracts/game/contract/src contracts/lottery/contract/src \
         --include=*.rs | grep -v "src/bin/schema.rs" || true)
if [ -z "$hits" ]; then
  ok "game / lottery 合约无 session 代码（无感登录已彻底剥离）"
else
  bad "仍有 session 引用:"; echo "$hits"
fi

# 1.2 消息结构不得含签名字段
if grep -qn "session_addr\|signature" contracts/game/contract/src/msg.rs \
                  contracts/lottery/contract/src/msg.rs; then
  bad "msg.rs 仍含 session_addr / signature 字段"
else
  ok "两份 msg.rs 无签名字段"
fi

# 1.3 依赖不得指向仓库之外
if grep -n "path = \"\.\./\.\./\.\." contracts/*/contract/Cargo.toml >/dev/null 2>&1; then
  bad "存在指向仓外的相对路径依赖（会导致克隆后编译失败）"
else
  ok "无仓外路径依赖（模板可独立克隆编译）"
fi

# 1.4 schema 生成器必须存在
for c in game/contract lottery/contract; do
  [ -f "contracts/$c/src/bin/schema.rs" ] \
    && ok "contracts/$c/src/bin/schema.rs 存在" \
    || bad "contracts/$c/src/bin/schema.rs 缺失（cargo build 会失败）"
done

# 1.5 许可证结构
for f in LICENSE NOTICE contracts/common/LICENSE contracts/common/NOTICE \
         contracts/lottery/LICENSE contracts/lottery/NOTICE; do
  [ -f "$f" ] && ok "$f 存在" || bad "$f 缺失"
done

# 1.6 工具链与 CI 文件
for f in rust-toolchain.toml .gitignore .github/workflows/ci.yml; do
  [ -f "$f" ] && ok "$f 存在" || bad "$f 缺失"
done

# 1.7 Cargo.lock（三个 crate 各自一份，故意不被 .gitignore 忽略）
for d in contracts/common contracts/game/contract contracts/lottery/contract; do
  if [ -f "$d/Cargo.lock" ]; then
    ok "$d/Cargo.lock 存在"
  else
    warn "$d/Cargo.lock 缺失 —— 跑 (cd $d && cargo generate-lockfile) 生成后提交"
  fi
done

# 1.8 .gitignore 不得忽略 Cargo.lock（可复现构建依赖它）
if grep -qE "^Cargo\.lock$" .gitignore; then
  bad ".gitignore 忽略了 Cargo.lock —— 会破坏可复现构建"
else
  ok ".gitignore 未忽略 Cargo.lock（可复现构建）"
fi

# 1.9 单测健全性：上游生产合约遗留过两个「永远失败」的坏测试
#     （overflow_guard 断言前提不可达、strong_team_wins_early_stop 数据构造与断言矛盾），
#     已在本模板修正。若这两个测试再次失败，说明有人改坏了战斗引擎 —— 必须阻断。
if grep -q "fn overflow_guard" contracts/game/contract/src/sanguo/battle/compute.rs 2>/dev/null \
   && grep -q "fn strong_team_wins_early_stop" contracts/game/contract/src/sanguo/battle/compute.rs 2>/dev/null; then
  ok "战斗引擎的两个关键单测存在（overflow_guard / strong_team_wins_early_stop）"
else
  bad "战斗引擎关键单测缺失 —— 请核对 compute.rs"
fi

echo
echo "=============================================================="
echo " 2. 工具链"
echo "=============================================================="
command -v cargo >/dev/null && ok "cargo: $(cargo --version)" || bad "未找到 cargo"
if [ -f rust-toolchain.toml ]; then
  ok "rust-toolchain.toml 存在（rustup 会自动切换到 1.69.0）"
  grep -q '1.69.0' rust-toolchain.toml \
    && ok "已钉死 1.69.0（链上 wasm 硬性要求：不能有 sign-ext 指令）" \
    || bad "rust-toolchain.toml 未钉 1.69.0"
fi
rustup toolchain list 2>/dev/null | grep -q "1.69" \
  && ok "1.69 工具链已安装（wasm 构建需要）" \
  || warn "未检测到 1.69 工具链，wasm 构建请先: rustup toolchain install 1.69.0"
rustup target list --installed --toolchain 1.69.0 2>/dev/null | grep -q "wasm32-unknown-unknown" \
  && ok "wasm32-unknown-unknown (1.69.0) 已安装" \
  || warn "1.69.0 未装 wasm target: rustup target add wasm32-unknown-unknown --toolchain 1.69.0"

echo
echo "=============================================================="
echo " 3. 构建与测试"
echo "=============================================================="

# check / test / schema 用宿主机默认工具链（较新 rustc 编译依赖更省事）；
# 只有链上 wasm 产物必须用 1.69.0（见第 4 节）。
build_one() {
  local dir="$1" name="$2" tc="$3"
  echo
  echo "--- $name (toolchain: $tc) ---"
  ( cd "$dir" && cargo "+$tc" check --message-format=short 2>&1 | tail -30 )
  if [ "${PIPESTATUS[0]}" -eq 0 ]; then ok "$name cargo check 通过"; else bad "$name cargo check 失败"; fi

  ( cd "$dir" && cargo "+$tc" test 2>&1 | tail -25 )
  if [ "${PIPESTATUS[0]}" -eq 0 ]; then ok "$name cargo test 通过"; else bad "$name cargo test 失败"; fi

  ( cd "$dir" && cargo "+$tc" run --quiet --bin schema 2>&1 | tail -10 )
  rc="${PIPESTATUS[0]}"
  # 产物结构：schema/<crate>.json（汇总）+ schema/raw/*.json（各消息变体）
  n_sum=$(ls "$dir"/schema/*.json 2>/dev/null | wc -l)
  n_raw=$(ls "$dir"/schema/raw/*.json 2>/dev/null | wc -l)
  if [ "$rc" -eq 0 ] && [ "$n_sum" -gt 0 ] && [ "$n_raw" -gt 0 ]; then
    ok "$name schema 生成成功（汇总 $n_sum 个 + raw $n_raw 个）"
  else
    bad "$name schema 未生成（退出码 $rc，汇总 $n_sum，raw $n_raw）"
  fi
}

# 宿主机工具链：优先用已装的稳定版，缺则退回默认
HOST_TC="$(rustup toolchain list 2>/dev/null | grep -E '^[a-zA-Z].*-x86_64-unknown-linux-gnu' | head -1 | awk '{print $1}')"
[ -z "${HOST_TC:-}" ] && HOST_TC="$(rustup default 2>/dev/null | awk '{print $1}')"
[ -z "${HOST_TC:-}" ] && HOST_TC="stable"
ok "宿主机构建工具链: $HOST_TC"

# 宿主机工具链：优先用已装的稳定版，缺则退回默认
HOST_TC="$(rustup toolchain list 2>/dev/null | grep -E '^[a-zA-Z].*-x86_64-unknown-linux-gnu' | head -1 | awk '{print $1}')"
[ -z "${HOST_TC:-}" ] && HOST_TC="$(rustup default 2>/dev/null | awk '{print $1}')"
[ -z "${HOST_TC:-}" ] && HOST_TC="stable"
ok "宿主机构建工具链: $HOST_TC"

# 3.0 先确保三份 Cargo.lock 存在（可复现构建的前提）
#
# ⚠️ 必须用 **1.69.0** 生成 lockfile，不能用宿主 stable：
#    cargo 1.78+ 生成的 lock 是 version = 4，而 cargo 1.69 只认 version = 3
#    （读 v4 会报 "lock file version 4 requires -Znext-lockfile-bump"），
#    第 4 节的 wasm 构建就会直接失败。1.69 生成的 v3 格式则新旧 cargo 都能读。
for d in contracts/common contracts/game/contract contracts/lottery/contract; do
  if [ -f "$d/Cargo.lock" ]; then
    ver="$(grep -m1 '^version = ' "$d/Cargo.lock" 2>/dev/null | awk '{print $3}' | tr -d '"')"
    if [ "$ver" = "4" ]; then
      warn "$d/Cargo.lock 是 v4 格式，cargo 1.69 读不了 → 重新用 1.69 生成"
      rm -f "$d/Cargo.lock"
    else
      ok "$d/Cargo.lock 已存在（v${ver:-3}）"
    fi
  fi
  if [ ! -f "$d/Cargo.lock" ]; then
    ( cd "$d" && cargo +1.69.0 generate-lockfile ) \
      && ok "$d/Cargo.lock 已生成（1.69.0，v3 格式）" \
      || warn "$d lockfile 生成失败（可能是 crates.io 网络不通 —— 见操作手册第 2 段的镜像配置）"
  fi
done

build_one contracts/game/contract   "游戏合约"   "$HOST_TC"
build_one contracts/lottery/contract "抽奖合约" "$HOST_TC"

echo
echo "=============================================================="
echo " 4. wasm 产物（需 1.69 工具链）"
echo "=============================================================="
# ⚠️ 必须带 --lib：
#    cosmwasm-schema 2.x 的 write_api! 宏在 wasm32 下会 compile_error，
#    且 #[derive(QueryResponses)] 生成的 impl 带 #[cfg(not(target_arch="wasm32"))]，
#    导致 src/bin/schema.rs 无法编到 wasm32。--lib 只编 lib，天然跳过该 bin。
for dir in contracts/game/contract contracts/lottery/contract; do
  # 先清掉旧产物，避免「上次残留的 wasm」被误判为本次构建成功
  rm -f "$dir"/target/wasm32-unknown-unknown/release/*.wasm 2>/dev/null || true
  ( cd "$dir" && cargo +1.69.0 build --release --lib --target wasm32-unknown-unknown 2>&1 | tail -8 )
  rc="${PIPESTATUS[0]}"
  if [ "$rc" -ne 0 ]; then
    bad "$dir wasm 编译失败（退出码 $rc）"
  elif ls "$dir"/target/wasm32-unknown-unknown/release/*.wasm >/dev/null 2>&1; then
    sz=$(du -h "$dir"/target/wasm32-unknown-unknown/release/*.wasm | cut -f1)
    ok "$dir wasm 产物已生成（$sz）"
  else
    bad "$dir wasm 产物未生成"
  fi
done

echo
echo "=============================================================="
if [ "$fail" -eq 0 ]; then
  echo -e "${GREEN} 全部检查通过${NC}"
else
  echo -e "${RED} 存在失败项，请查看上方 ✗ 标记${NC}"
fi
echo "=============================================================="
exit $fail
