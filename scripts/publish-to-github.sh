#!/usr/bin/env bash
# =============================================================================
# cosmwasm-game-template —— 一键发布到 GitHub
#
# 用法（WSL）：
#   cd /mnt/g/paxi-tongyong-heyue/dao-tian/cosmwasm-game-template-DRAFT
#   bash scripts/publish-to-github.sh
#
# 前置条件（只需做一次）：
#   1) 在 GitHub 网页上创建一个**空**仓库（不要勾选 README / .gitignore / License）：
#        仓库名：cosmwasm-game-template
#        可见性：Public
#        地址：  https://github.com/moxiaomo856/cosmwasm-game-template
#      （用网页创建才能拿到干净的空仓库，本脚本直接往里推）
#   2) 确认 git 凭据可用（能 push）：
#        git ls-remote https://github.com/moxiaomo856/cosmwasm-game-template.git
#      若提示输入用户名密码，用 Personal Access Token 作为密码。
#
# 本脚本只做：生成 lockfile → 提交 → 推送。不会删除任何文件，不会 force push。
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

REPO_SLUG="moxiaomo856/cosmwasm-game-template"
REMOTE="https://github.com/${REPO_SLUG}.git"
GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
ok()   { echo -e "${GREEN}✓${NC} $*"; }
warn() { echo -e "${YELLOW}!${NC} $*"; }
die()  { echo -e "${RED}✗${NC} $*"; exit 1; }

echo "=============================================================="
echo " 发布 ${REPO_SLUG}"
echo "=============================================================="

command -v git >/dev/null || die "未找到 git"
ok "git: $(git --version)"

# --- 1. 预检：远端是否可访问 -----------------------------------------------
echo
echo "[1/5] 检查远端"
if git ls-remote "$REMOTE" >/dev/null 2>&1; then
  ok "远端可访问"
else
  die "远端不可访问。请先在 GitHub 网页创建空仓库 ${REPO_SLUG}（不要勾选任何初始化文件），
     并确认 git 凭据可用：
       git ls-remote ${REMOTE}"
fi

# --- 2. 生成 Cargo.lock ----------------------------------------------------
echo
echo "[2/5] 生成 Cargo.lock（三个 crate 相互独立，各自一份）"
for d in contracts/common contracts/game/contract contracts/lottery/contract; do
  if [ -f "$d/Cargo.lock" ]; then
    ok "$d/Cargo.lock 已存在"
  else
    ( cd "$d" && cargo generate-lockfile ) || die "lockfile 生成失败：$d"
    ok "$d/Cargo.lock 已生成"
  fi
done
warn "请确认刚生成的 Cargo.lock 一并提交 —— 本模板的可复现构建依赖它。"

# --- 3. 结构自检 -----------------------------------------------------------
echo
echo "[3/5] 发布前结构自检"
if grep -rq "session\|Session" contracts/game/contract/src contracts/lottery/contract/src \
     --include=*.rs 2>/dev/null; then
  hits=$(grep -rn "session\|Session" contracts/game/contract/src contracts/lottery/contract/src \
         --include=*.rs | grep -v "src/bin/schema.rs" || true)
  if [ -n "$hits" ]; then
    warn "发现 session 引用（确认是否只剩注释）："
    echo "$hits"
  fi
fi
grep -rn "path = \"\.\./\.\./\.\." contracts/*/contract/Cargo.toml >/dev/null 2>&1 \
  && die "存在仓外路径依赖，克隆后会编译失败" \
  || ok "无仓外路径依赖"

# --- 3.5 泄漏门禁：内部路径 / 项目方地址 / 无感残留 一律不得进公开仓 --------
leak_pattern='tong yong he yue xin|sanguo-heguiban|sanguoyouxi|choujiang-|ptkc-talk-|moxiaomo856/jinliujie|18701655949|17468711383|paxi1rdarmm997hqwfdgl9wvnpffe28zmex3kfyg7xd|paxi1qvrmsft|paxi194kpjqhyz7re2g749lc2030cgeg4sql5ldvyem'
leaks="$(grep -rInE "$leak_pattern" . \
  --exclude-dir=.git --exclude-dir=target --exclude-dir=schema \
  --exclude=publish-to-github.sh 2>/dev/null || true)"
if [ -n "$leaks" ]; then
  warn "检测到内部信息泄漏，发布中止："
  echo "$leaks"
  die "请先清理上述命中行，或在 scripts/publish-to-github.sh 的 leak_pattern 中确认是否误报"
fi
ok "泄漏扫描通过（无内部路径 / 项目方地址 / 私人标识）"

# 3.6 不得把 session（无感鉴权）代码或签名字段带进公开仓
#     （schema.rs 里有一句「消息结构中不存在 session_addr …」的注释，须排除，否则误杀）
if grep -rnE "session_addr|validate_and_consume_session|SIGN_DOMAIN" \
     contracts/game/contract/src contracts/lottery/contract/src --include=*.rs 2>/dev/null \
     | grep -v "src/bin/schema.rs" | grep -q .; then
  die "检测到 session 签名字段残留，公开模板不得包含无感鉴权实现"
fi
ok "无 session 签名字段残留"
for f in LICENSE NOTICE rust-toolchain.toml .gitignore .github/workflows/ci.yml; do
  [ -f "$f" ] || die "缺少 $f"
done
ok "必要文件齐备"

# --- 4. 提交 ---------------------------------------------------------------
echo
echo "[4/5] 提交"
if [ ! -d .git ]; then
  git init -q -b main
  ok "已 git init（branch: main）"
fi
git remote remove origin >/dev/null 2>&1 || true
git remote add origin "$REMOTE"

git add -A
if git diff --cached --quiet; then
  warn "没有待提交的变更"
else
  git -c user.name="${GIT_AUTHOR_NAME:-moxiaomo856}" \
      -c user.email="${GIT_AUTHOR_EMAIL:-moxiaomo856@users.noreply.github.com}" \
      commit -q -m "Initial release: cosmwasm-game-template

- contracts/game: card-game contract (MIT)
- contracts/lottery: lottery contract (Apache-2.0, derived from upstream)
- contracts/common: vendored paxi-common shared crate (Apache-2.0)
- Authentication: transaction sender signs directly; the session-key
  invisible-UX module is intentionally not shipped (see docs/)
- Dual license: MIT for original work, Apache-2.0 for derived directories
  (see LICENSE and NOTICE)

Scope change vs. the funding proposal is disclosed in
docs/scope-clarification.md."
  ok "已提交"
fi

# --- 5. 推送 ---------------------------------------------------------------
echo
echo "[5/5] 推送到 main"
git push -u origin main
ok "推送完成"

echo
echo "=============================================================="
echo -e "${GREEN} 发布完成${NC}"
echo "  https://github.com/${REPO_SLUG}"
echo
echo "  发布后请做："
echo "   1) 打开仓库 Settings → Pages（若要挂前端 demo）"
echo "   2) 把 docs/scope-clarification.md 的内容发到社区（Discord/论坛）"
echo "   3) 起草第一篇 build log（进度 / 测试网地址 / 截图）"
echo "=============================================================="
