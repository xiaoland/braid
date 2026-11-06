# Agent Profile 与原生材料

Profile 定义根 Agent 的 adapter、model、可选 reasoning、指令及支持的材料选择；
只有 Pi Profile 额外引用 `llm_providers` 中的 provider。Codex 通过自己的
provider home 认证，Profile 不声明 LLM provider 或 API key 文件。GitHub
多 repository instance 中，每个 Profile 还必须声明自己的 `repository` 和
显式 `workspace`；Profile id 在 instance 内唯一，不能用同一 id 隐含不同仓库
的指令。Repository 只限定调度、工作区、恢复和写入边界，不共享 Context。
Runtime 定义
可执行入口、adapter 版本及锁定插件安装。Factory 的 preset、variant
和实验配方由消费方装配，不导入 Braid 的配置目录。Pi 原生 subagent 角色是根
Agent 的局部执行能力，不是可独立指派的 Braid Profile。

`user_instructions` 保持现有配置字段。消费方可以从独立的 `instructions.md`
装配它，但不能将 SKILL.md 或 references 正文混入这些角色指令；Braid 不接管
Factory 的模板生成流程，也不默认要求其 SVC 技能、分支流程或原生角色配方。

当前执行器使用第一个带 `issue` tag 的 Profile 和 `profile_selection.default_pr_profile`
指定的 PR Profile。配置中存在其它 Profile 不表示有对应 driver；不要将 catalog
可查看解释为任意 Profile 可执行。

## 选择 Pi 材料

在已有 `[[profiles]]` 中可以配置以下可选字段；相对路径按 Harness 配置文件所在
目录解析。`skills` 的每项是一个技能目录、直接的 `SKILL.md`，或直接子目录各自
包含 `SKILL.md` 的技能 catalog；catalog 按稳定目录名展开，不递归寻找更深层的
技能集合。其它 adapter 没有此材料消费接口，非空配置会明确拒绝。

```toml
skills = ["materials/skills/example"]
native_template = "materials/pi-issue"
```

技能独立保存，包含按需读取的 references 和工具资源。Pi 使用显式技能发现入口，
提示词只包含名称、description 和路径；Braid 不把 SKILL.md 或 reference 正文拼入
system、profile 或任务输入。已选锁定插件附带的技能仍由安装 manifest 决定；
项目或用户目录中未选择的技能不自动加入。仓库的 [braid-collaboration](../../skills/README.md)
是可选方法材料，Braid 不自动安装或启用它。

Pi 模板是材料源，允许 `settings.json` 和 `agents/*.md`，不携带 auth、原生会话、
缓存或临时文件。未指定模板时，已有 `RuntimeEntry.home` 的受支持语义文件作为
只读来源，不能将整个 home 当作模板复制。没有这两种来源时使用空模板。每个执行
工作树持有独立、持久的 Braid 原生材料目录，避免 Issue 与 PR 改写共享 home；
恢复不会用当前模板覆盖旧材料。

`models.json` 不再是材料输入：Pi adapter 从 `llm_providers` 声明派生它。每个被
Pi Profile 引用的条目需要 `base_url`、可选 `api`，以及至少包含 Profile `model`
的 `[[llm_providers.models]]` 声明（`id`、`name`、`reasoning`、`input`、
`context_window`、`max_tokens`、`thinking_level_map`、`compat` 均按声明渲染）。渲染出的
`apiKey` 只在 `credential.kind = "api_key"` 时出现并引用 Braid 管理的
`$BRAID_PROVIDER_API_KEY`；模板或 runtime home 中出现 `models.json` 会被明确拒绝并
指向这份声明。根模型及推理仍由 Profile 决定，不是第二套根模型选择；凭据由
`llm_providers` 条目的 `credential` 声明。`settings.json` 只支持有实际消费者的设置，不能另行选择根模型、
凭据、扩展、技能或动态安装 package。扩展由 `RuntimeEntry.native_runtime` 的锁定
manifest 提供。原生子角色的职责、模型、工具与继承范围由受支持的角色材料及
Pi 插件决定；Braid 不把这些角色提升为 Issue/PR 负责人，也不承诺仓库代码的
安全沙箱。

## Provider credential

每个 `llm_providers` 条目必须显式声明凭据，三种 kind 覆盖全部情况：

```toml
[[llm_providers]]
id = "example-llm"
credential = { kind = "api_key", file = "/absolute/path/to/secrets/example-llm.toml" }
# 或：credential = { kind = "oauth", file = "/absolute/path/to/secrets/kimi.toml" }
# 或：credential = { kind = "none" }（显式 keyless，不引用任何文件）
```

凭据文件是 Braid 自己的 schema，不是 runtime 的文件格式。`api_key` 文件仍是
`provider_api_key = "..."`；`oauth` 文件是 token set：

```toml
access = "..."
refresh = "..."
expires = 4102444800000   # epoch 毫秒
account = "..."           # 可选，仅诊断展示
```

Pi adapter 是唯一位于 runtime 格式边界内的层：`api_key` 凭据被读取后以
`BRAID_PROVIDER_API_KEY` 注入对应 Pi 子进程（`models.json.apiKey` 固定引用该
名称，且仅在 kind 为 `api_key` 时允许）；`oauth` token set 被渲染为 session home
的 `auth.json`，`none` 不注入任何东西。凭据是静态秘密，Braid 从不修改凭据文件；
过期或轮换由显式替换触发，内容 hash 变化会改变 Profile 有效版本并让旧 session
明确 blocked。kind 与内容摘要可见于 `braid profile inspect`，凭据值永不展示。

既有 Pi `auth.json` 不要手工复制到任何 runtime home——材料快照从不携带它。
用一次性显式 import 转换进 Braid secrets：

```sh
braid provider import-auth <PROVIDER_ID> --from /path/to/auth.json
```

服务 URL 不携带 userinfo 凭据。Header
保留 Pi 的原生解析能力，识别为认证用途的 header 必须引用同一固定名称，普通协议
header 可以声明非秘密值；配置者不得借其它
字段写入凭据。本材料合同支持 `$ENV_NAME`、`${ENV_NAME}` 及常见 `Bearer` 引用；
未知环境引用、其它混合模板和 `!command` 明确拒绝，后者会额外执行未受控 shell。
正常版本或部署 query 保留，明确的凭据 query 不允许。模板中的非空原生角色
extensions 会明确拒绝，不能绕开锁定安装。
要消费 `agents/*.md` 或 `subagents` 设置，受控 Pi 安装须显式选择 `subagents` 插件；
模板不负责安装或启用它。外部基础 Pi 的扩展发现关闭，不提供这项原生角色能力。
角色 `skillPath: "@SKILLS@"` 指向本次 Profile 技能快照的 catalog，按技能名称
选择的条目须在 `skills` 声明中，不能引用未冻结的外部材料。

## 查看实际选择与恢复身份

```sh
braid profile inspect --config /absolute/path/to/config.toml PROFILE_ID
braid profile inspect --config /absolute/path/to/config.toml PROFILE_ID --json
```

查看结果保留 Profile 声明，并提供有效版本、摘要和选择的材料身份。它不显示
原生文件正文或凭据值，也不证明已发起模型推理。实际启动记录用于核对对应会话
消费的材料；原生子角色是否执行仍需其实际启动、结果和模型记录。

有效 Profile 身份包含声明、adapter 合同、锁定安装 manifest、所选技能与模板
内容及非秘密服务路由。临时 execution、工作树和派生 home 路径及凭据值不作为
语义材料身份。材料变化会改变有效版本；恢复旧版本会明确 blocked，不静默在
新材料下续用旧历史。应先完成旧执行停止，再按现有公开重新指派或新运行流程
消费新配置，不能修改 ledger 或旧 home 标记制造兼容。

Pi 自己保存的根模型偏好不成为第二权威：Profile 显式指定的 model 及根
provider 覆盖对应偏好；reasoning 只在声明时覆盖。快照设置按 JSON 语义核对，避免仅格式变化阻断恢复；
其它行为键、角色与技能内容变化仍拒绝复用。原生运行状态不作为声明材料复制。

这次身份合同补全后，旧版仅按 Profile JSON 生成的版本也可能不兼容；保留旧
运行、原生记录与材料，不将新配置启动称为原生历史接续。配置查看、编译与
无推理装载只证明各自边界，不代替真实模型协作和产品验收。
