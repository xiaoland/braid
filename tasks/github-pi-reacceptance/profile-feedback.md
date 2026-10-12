# 协作技能与 Profile 反哺

2026-10-12，按用户“继续反哺”的指示，从 Factory26 `f3963f8` 核对材料及实现，
融合到独立 Braid `4d459e6` 之后的同一 PR 分支，源码候选为 `f82ee88`，已推送到 Draft PR148。没有新模型运行、生成应用修改
或 GitHub 业务对象操作，既有 ARC/GitHub 资格结论不变。

## 已采用的通用能力

`skills/braid-collaboration/` 保留 SKILL.md 与组织工作、交接与变化、接受与关闭、
完整例子四份 reference。操作部分按独立 Braid 的双上游、serve/run、对象 CLI
和 Console/OTLP 调整；包内引用不依赖整个软件 checkout 或相邻 SVC 目录。
技能独立发现、按需读取，不安装、不编译嵌入、不加入默认 Profile。

普通 Profile catalog、当前 Issue/默认 PR 执行者、根模型与推理的唯一来源已经
迁入，本轮不重复实现，也不扩大任意 Profile driver。新增 Pi 的显式 skills 与
native_template，采用真正的原生技能发现、模型 descriptor、设置和角色材料
消费者；运行安装仍拥有插件和可执行入口。每个原生会话保留独立 home 与索引，
冻结完整技能资源及语义模板，恢复复用核对文件和设置语义，禁止静默覆写。

有效 Profile 版本覆盖声明 source workspace、adapter 合同及 schema pins、锁定
安装 manifest、所选材料内容和非秘密服务路由，排除派生执行路径与凭据值。
现有 Issue/PR 恢复 revision 门控继续明确阻断不兼容历史。新建、恢复及延迟
物理启动共用 Pi get_commands 技能校验；每个声明技能须唯一且来自本会话快照。
Profile id/revision/digest 加入会话、生命周期及 provider payload 的 OTLP 身份。

Factory preset/variant、模型费用配方、强制 SVC/vision/reviewer/cleaner、develop/main
分支流程和 ARC/Lab 仍归消费方；没有复制它们为 Braid 默认行为。Braid 也没有
添加空 tools/MCP 管理器或 Codex 技能装配接口。能力及限制归
[Profile 指南](../../docs/user-manual/profiles.md)，恢复合同归技术说明。

## 反馈与限定

所有 Mac 操作材料位于 `/Volumes/WorkSSD/Development/braid-profile-feedback-20261012/`。
单任务 `cargo build --locked --offline -j1` 最终27.88秒通过，无 warning；binary
SHA-256 为 `ef6275fb1dd740032c6b94f76539108170745c0fe02c1740f4ca3101a62df093`，
原件 `raw/build-qualified.log`。中间未使用变量 warning、get_commands helper
误入 trait impl 的编译失败及修复记录保留，没有将中间 binary 当作最终候选。
没有设施测试、自检或模型请求。

真实 `profile inspect --json` 显示 MiniMax-M3、所选技能及模板材料。
修改同路径 settings 默认工具集合，摘要/版本从 `829f6ad4…` /
`588272450525796732` 变为 `1f66…` / `141410568920633989`，还原后返回原值。
这证明身份覆盖该材料变化，不证明实际历史 resume 门控已执行。

公开新建/指派 Issue #2 后会话 idle、turn_count=0、provider_resume_count=0，
Pi 参数包含 --no-skills 及快照 --skill，原生 home 保存完整角色、技能、模型
和设置材料。成功 idle 是本候选 get_commands 启动门控通过的行为证据；本次未
启动 Collector，单独发现回执未保留，OTLP 导出和 flush 错误仍保留原件。
该次 Harness/Pi/platform 全部正常停止。

随后通过 Console 实际 Collector（14318）与内建平台（18991）补采，采样1，
新建/指派 Issue #3，仍为 idle/0 turn。公开 API 保存88条 records、20个原始
protobuf batches，接收错误0；不从这些计数推断整体无遗漏。record #44 的官方
get_commands 返回 success=true，技能名称及 sourceInfo.path 指向本会话快照；
#47 的 profile_resources 包含同会话 id、Profile id/digest 和技能路径，trace #76
包含相同会话的 Profile id、revision 与 digest。原件及摘要见
`raw/otlp-records-qualified.json`、`raw/otlp-batches/`、
`raw/otlp-batch-manifest.json`、`raw/otlp-profile-session-summary.json`。

这次 Harness 启动还按既有策略接续了前两次空闲会话，保持0 turn，不能等同于
继承真实模型历史。Harness、Console及其托管平台正常退出0；只读回收对象回执
所需的临时平台也退出0。14318/18991/18992/18993均无监听，所属 PID 全部退出；
证据 `raw/otlp-process-pid-evidence.json`、`raw/otlp-status-before-stop.json`、
`raw/otlp-issue-receipt.json`、`raw/otlp-assign-receipt.json`。

首次 idle 仅产生 Context 旁文件，没有模型历史，也没有为取证发送空推理。
真实历史恢复、材料变化后的实际恢复阻断、原生角色实际调用、Codex 和模型质量
均未在本轮验收，不把编译、idle 或技能发现等同于这些结果。
