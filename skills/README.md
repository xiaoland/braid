# 独立 Agent Skills

本目录保存可由 Agent 按需读取的独立方法材料。它们与 Braid 软件分开消费，不自动安装、加载、编译嵌入或加入默认 Profile，也不将 SKILL.md 及 references 正文内联到提示词。使用者将所需技能目录交给自己的 Agent Skills 发现机制；入口只保留名称、description 和路径，相关 reference 在需要时读取。

| 名称 | 用途 | 入口 |
| --- | --- | --- |
| braid-collaboration | 组织成果责任、接手剩余义务、判断变化影响、核对成员执行及采用与完成。 | [SKILL.md](braid-collaboration/SKILL.md) |

`braid-collaboration` 反哺自 Factory26 的 `materials/skills/braid-collaboration/`（来源提交 `f3963f89e6f0250ea78a2702ebe2a6fcc5d46ba8`），保留四份专题材料，并按独立 Braid 的 GitHub/内建上游、serve/run 及现有 Console 接口调整操作部分。技能本身不授予模型运行或对象修改权限，不要求安装其它 SVC 技能；可选互补方法通过当前环境的实际技能清单发现。软件操作合同仍由 Braid 对应版本的项目文档持有，不由技能创建新接口；复制 `braid-collaboration/` 目录即可保留技能及其全部方法 references，无需复制软件源码。
