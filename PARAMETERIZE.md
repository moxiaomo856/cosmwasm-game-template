# 参数化清单（公开前必做）

> 把下面「游戏专属硬编码」抽成 `InstantiateMsg` / 配置文件 / 示例数据，才能当模板复用。
> 每项标 `需核对` 的，等你在 WSL 编译时确认实际常量。
>
> **状态：2026-10-21 更新**。已完成项标 ☑，未完成标 ☐。

---

## ☑ 已完成（2026-10-21）

- [x] **vendor `paxi-common` → `contracts/common/`**：原先 `paxi-common` 用相对路径指向
      **仓外**的上游工作区包，复制到模板后层级对不上、模板无法编译。
      已复制为自包含 crate，workspace 继承全部展开为上游精确锁定版本。
- [x] **移除 `paxi-common/src/session.rs`**：无感实现不对外提供。
      `lib.rs` 相应去掉 `pub mod session;` 与 `pub use session::{...}`。
- [x] **剥离两份合约的 session 鉴权**：
      - 游戏合约：删除 `src/session/`（register / validate / revoke）、
        `validate_and_consume_session`、`SESSIONS` 映射、8 个 session 错误变体、
        `RegisterSession`/`RevokeSession` 消息与查询臂；
        玩家身份统一由 `src/sanguo/helpers.rs` 的 `sanguo_auth()` 返回 `info.sender`。
      - 抽奖合约：删除 `src/session/`、`SessionAuth`、`RegisterSession`/`RevokeSession`、
        `USER_SESSIONS` / `MAX_SESSIONS_PER_USER` / `SIGN_DOMAIN`；
        玩家身份由 `sender: &Addr` 传入。
      - 消息结构中 `session_addr` / `nonce` / `signature` 字段全部清除。
- [x] **补 `src/bin/schema.rs`**（两份合约）：原先 `Cargo.toml` 声明了 `[[bin]] schema`
      但文件缺失，`cargo build` 必失败。
- [x] **清理死依赖**：游戏合约移除 `bech32` / `ripemd` / `hex`（session 专用）；
      抽奖合约移除 `cw-utils` 与整个 `[dev-dependencies]`（`k256` / `cw-multi-test` / `anyhow`）。
      **保留** `cosmwasm-crypto` / `cosmwasm-derive` / `schemars` 的精确锁定——
      它们虽无直接引用，但放任解析会拉到 2.3.x（edition2024），Rust 1.69 编不过。
- [x] **双许可证结构**：根 `LICENSE`（MIT）+ 根 `NOTICE` +
      `contracts/common/{LICENSE,NOTICE}`（Apache-2.0）+ `contracts/lottery/{LICENSE,NOTICE}`（Apache-2.0）。
- [x] **文档**：`docs/invisible-ux-boundaries.md`、`docs/security-notes.md`、
      `docs/scope-clarification.md`（对社区的范围变更披露）。
- [x] **发布前清理（公开仓不能夹带生产信息）**：
      - 🔴 **硬编码的三个项目方国库地址已移除**（`contract.rs` instantiate）。原先把
        具体地址写死为默认值，部署者会把原作者的国库一起带进自己的合约。
        现改为**不设默认**，由管理员部署后调用 `SanguoSetProjectWallets` 配置；
        未配置时 `pick_project_wallet` 返回 `SanguoNoProjectWallet`（错误文案已提示
        「admin must call SanguoSetProjectWallets」），属显式失败而非静默打款。
      - 链级 Swap 模块地址抽为 `state::DEFAULT_SWAP_MODULE` 常量（属链基础设施，
        保留合理；部署到其它链用 `InstantiateMsg.swap_module` 覆盖）。
      - **删除生产运维手册** `contracts/lottery/contract/部署升级说明.md`
        （含线上主网合约地址 + 内部前端路径 + 内部工作区名），
        替换为通用的 `docs/deployment.md`——并把其中**可复用的**「升级前清场流程」
        （`SanguoLegacyStatus` / `AdminSweep` / 2 天提现窗口 / 防扫玩家资产的硬护栏）
        以通用形式保留了下来。
      - 4 个 README 重写为公开文档（原先是 DRAFT 占位 + 生产审计快照，
        且引用了模板里已不存在的文件如 `../../app.js`、`session/register.rs`）。
      - 清理所有内部目录名与内部工作流措辞（原工作区路径、生产部署说明、内部审计轮次）。
- [x] **发布基建**：`rust-toolchain.toml`（钉 1.69.0 + wasm target）、
      `.gitignore`（**故意不忽略 Cargo.lock**）、`.github/workflows/ci.yml`、
      `scripts/verify-build.sh`、`scripts/publish-to-github.sh`。
- [x] **仓库落位 `moxiaomo856/cosmwasm-game-template`**：README / `scope-clarification.md`
      / `proposal.md` §9 注记 + §11.4 均已如实记录「提案写的是 org，实际落在个人账号，
      项目与任何 Paxi 官方组织无隶属或背书关系」。

---

## ☐ 三国合约 `contracts/game`

- [ ] `SG_SKILLS`(60) / `SG_BONDS`(23) 角色/连携表 → 抽成 `examples/` 示例数据或 config（前端也有同名镜像，需同步）
- [ ] `FRAG_UNIT=[1,5,20,80]` 碎片汇率 → 放入 `InstantiateMsg` 可配
- [ ] 关卡/战斗数值、阵营/官职等游戏专属逻辑 → 抽示例，留接口
- [ ] cw721 NFT 字段（姓名/称号/阵营）→ 保留 trait，示例数据外置
- [ ] 确认 `SanguoBattlePvp`/`SanguoBattleAi` 合并 API 的 `card_ids` 必填约定已在 README 注明（前端曾因缺字段报错）
- [ ] `round_id` 现在**不参与任何签名**（session 移除后它只剩幂等/追溯用途）。
      当前保留在 `ExecuteMsg` 里是出于链上 ABI 稳定性考虑（Rust 侧已重命名为 `_round_id`）。
      → 决定：保留并在文档中说明其语义，还是从消息中彻底移除
- [ ] 抽卡 / 碎片加购 / 商店等入口的**费率与日限** → 可配（当前在 `params` 里，但仍是三国专属数值）
- [ ] `DIFF_BOOST_BP`（AI 难度）→ 可配
- [ ] 13 个改动前既有的死错误变体（`EmptyAdmins` / `GameEngineConfigExists` /
      `Prc20ApproveFailed` / `SanguoBattleNotFound` / `SanguoNeed3Cards` 等）→ 清理或补齐使用
- [ ] 8 处既有的未使用 import（`contract.rs` 的 `AllowanceResponse`/`BalanceResponse`/`GAME_LIMITS`、
      `admin/config.rs` 的 `TOTAL_TREASURY`、`assets/*` 的 `Coin`/`Addr`、
      `sanguo/helpers.rs` 的 `empty_addr`）→ 清理
- [ ] 验证 `SanguoBattlePvp`→action `battle_pvp`、`SanguoBattleAi`→action `battle_ai`
      的合并 API 约定在模板中仍然成立

## ☐ 抽奖合约 `contracts/lottery`

- [ ] 票价、档位（TIER 配置）、奖池代币 denom → `InstantiateMsg` 可配
      （当前 `tier_spec()` 是源码常量）
- [ ] 双代币 `is_zero()` 守卫保留（PAXI/TKCC）——已保留，需在文档中说明
- [ ] ⚠️ **随机数来源**：本模板**已实现 commit-reveal**（`RandomSource::CommitReveal`）
      + 区块哈希兜底（`BlockHashFallback`），VRF 仅为预留枚举分支。
      → `docs/security-notes.md` 已如实标注兜底路径的 last-actor 攻击与出块者影响力风险。
      → 仍需：把「大额奖池禁用兜底」的约束写进部署检查清单
- [ ] `template.rs` 的模板化逻辑保留并作为卖点（已在 v2 中存在）
- [ ] 资金流：奖池充值/开奖/发放路径 → 文档画清
- [ ] 单币(danbi)/双币(shuangbi)差异 → 抽成参数，避免两份拷贝
- [ ] 11 个单元测试中 `creator_cannot_join_own_lottery` 已**弱化**
      （直签模型下 sender==creator 结构性成立，不再能测跨身份场景）。
      → 若要保留该覆盖，需自行实现测试用 session 层

## ☐ 前端 `frontend-demo`（待复制 paxi-card-game + cobra）

- [ ] 钱包连接抽象 → invisible-ux 模块
- [ ] feegrant 代付（**不随本模板开源**，见 `docs/invisible-ux-boundaries.md`）
- [ ] `HUB_BUILD` / `?v=` 缓存破坏等工程细节 → 抽配置
- [ ] 游戏专属 UI 文案 → 示例化

## ☐ 工程与发布

- [x] `rust-toolchain.toml`（钉死 1.69.0 + wasm target）+ `.gitignore`（**故意不忽略 Cargo.lock**）
- [x] `.github/workflows/ci.yml`：check / test / schema 生成 / wasm 构建 / 体积报告 / 导入段检查
      （clippy 设为 advisory 非阻断，因上游遗留告警待清理）
- [x] `scripts/verify-build.sh`（本地一键自检 + 构建）、`scripts/publish-to-github.sh`（一键发布）
- [ ] **生成并提交三份 `Cargo.lock`**（`cargo generate-lockfile`）——可复现构建的前提
- [ ] WSL `cargo build` 两合约均通过（你本地编译，我不跑 cargo）
      —— **当前状态：尚未编译验证**（本环境无 Rust 工具链，且 `wsl.exe` 被安全策略拦截）
- [ ] `cargo test` 基础用例通过（游戏 4 个内嵌测试模块 + 抽奖 11 个用例）
- [ ] `cargo run --bin schema` 能生成 JSON schema
- [ ] `cargo clippy` 告警清零（当前为 advisory；遗留 8 处未使用 import + 13 个死错误变体）
- [ ] 用 `../AI提示词_整理代码文档.md` 生成 `docs/` 剩余 6 篇
      （architecture / contracts / integration-guide / deployment / faq / maintenance）
- [ ] 测试网部署 + 2 周监控（地址写进 build log）
- [ ] 前端 demo（`frontend-demo/` 目前只有 README 占位）
- [ ] 公开仓库创建：`github.com/moxiaomo856/cosmwasm-game-template`（提案通过后 7 天内）
      + 同步 `scope-clarification.md` 到社区（Discord / 论坛）
- [ ] `scripts/deploy-testnet.sh` / `scripts/store-code.sh`（部署脚本待写）
