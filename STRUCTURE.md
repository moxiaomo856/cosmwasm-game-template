# 目录结构（公开模板目标形态）

```
cosmwasm-game-template/
├── README.md                      # 公开模板主说明
├── LICENSE                        # MIT（覆盖自研部分：游戏合约 / docs / 前端 / scripts）
├── NOTICE                         # 双许可证结构说明 + 提案措辞差异披露 + 无感边界说明
├── PARAMETERIZE.md                # 参数化待办清单（公开前必做）
├── STRUCTURE.md                   # 本文件
├── rust-toolchain.toml            # 钉死 Rust 1.69.0 + wasm32 target（链上产物硬性要求）
├── .gitignore                     # 注意：故意 **不** 忽略 Cargo.lock
├── .github/
│   └── workflows/ci.yml           # CI：check / test / schema / wasm 构建 / 导入段检查
│
├── contracts/
│   ├── common/                    # 共享 crate（Apache-2.0，vendor 自 paxi-common）
│   │   ├── Cargo.toml             #   workspace 继承已展开为精确锁定版本
│   │   ├── LICENSE                #   Apache-2.0
│   │   ├── NOTICE                 #   上游出处 + 本副本的修改说明
│   │   └── src/
│   │       ├── admin.rs           #   管理员白名单 + 多签提案数据结构
│   │       ├── balance.rs         #   内部余额账本
│   │       ├── error.rs
│   │       ├── lib.rs             #   导出面（不含 session）
│   │       ├── prc20.rs           #   PRC-20（CW20 兼容）助手
│   │       ├── random.rs          #   commit-reveal 随机源 + 不放回抽样
│   │       └── tkcc.rs            #   外部 PRC-20 代币配置与销毁模式
│   │
│   ├── game/                      # 卡牌游戏合约（MIT）
│   │   ├── Cargo.toml
│   │   └── contract/
│   │       ├── .cargo/config.toml #   MSRV 回退解析器（必须随仓库提交）
│   │       ├── Cargo.toml
│   │       └── src/
│   │           ├── bin/schema.rs  #   JSON Schema 生成器
│   │           ├── contract.rs    #   instantiate / execute / query / migrate
│   │           ├── msg.rs         #   InstantiateMsg / ExecuteMsg / QueryMsg
│   │           ├── state.rs
│   │           ├── error.rs
│   │           ├── helpers.rs
│   │           ├── admin/         #   配置与迁移
│   │           ├── assets/        #   PRC-20 充值通道（deposit/withdraw/swap）
│   │           ├── games/         #   通用游戏引擎 + 限额
│   │           └── sanguo/        #   游戏域：battle / exec / gift / query / state
│   │
│   └── lottery/                   # 抽奖合约（Apache-2.0，源自上游 v2）
│       ├── LICENSE                #   Apache-2.0
│       ├── NOTICE                 #   出处 + 修改说明
│       ├── README.md
│       └── contract/
│           ├── .cargo/config.toml #   MSRV 回退解析器
│           ├── Cargo.toml
│           └── src/
│               ├── bin/schema.rs  #   JSON Schema 生成器
│               ├── contract.rs
│               ├── lottery.rs     #   建池 / 参与 / 开奖 / 领取 / 退款
│               ├── template.rs    #   平台模板池（哈希链揭示 + 轮换）
│               ├── msg.rs
│               ├── state.rs
│               ├── helpers.rs
│               ├── error.rs
│               └── tests.rs       #   11 个单元测试（直签模型）
│
├── frontend-demo/                 # React 参考前端（游戏 + 抽奖）
├── docs/
│   ├── invisible-ux-boundaries.md # 无感能力边界 + 信任模型 + 自建鉴权接入点
│   ├── security-notes.md          # 随机数 / 残余风险 / 管理员信任假设 / 审计状态
│   ├── scope-clarification.md     # 相对获批提案的范围变更披露（对社区）
│   ├── deployment.md              # 构建 / store / instantiate / migrate / 清场流程
│   └── ...                        # architecture / contracts / integration / faq / maintenance
└── scripts/
    ├── verify-build.sh            # 本地一键自检 + 构建（WSL）
    ├── publish-to-github.sh       # 生成 lockfile → 提交 → 推送到 moxiaomo856/cosmwasm-game-template
    ├── deploy-testnet.sh          # 部署与测试网脚本（待写）
    └── store-code.sh              # store code 脚本（待写）
```

> **三个 crate 相互独立，没有根 workspace。** 各自持有一份 `Cargo.lock`。
> 这样每个 crate 都能被单独复制走、单独使用，对一个「模板」项目更友好；
> 代价是 lockfile 要分别维护三份。

## 与早期骨架的差异（2026-10-21 更新）

| 变化 | 说明 |
|------|------|
| `contracts/common/` 新增 | 原本 `paxi-common` 是指向仓库外的相对路径依赖，复制进模板后路径断裂、无法编译。现已 vendor 为自包含 crate，并**移除 `session.rs`**（无感实现不公开） |
| `src/session/` 全部删除 | 游戏合约内联的 session 模块（register / validate / revoke）与抽奖合约的 `session/mod.rs` 均已移除，改为 `info.sender` 直签 |
| `src/bin/schema.rs` 新增 | 两份合约原先在 `Cargo.toml` 里声明了 `[[bin]] schema` 但文件缺失，`cargo build` 必失败。现已补齐 |
| `LICENSE` / `NOTICE` 新增 | 双许可证结构：自研部分 MIT，vendor 与衍生部分保留 Apache-2.0 |
