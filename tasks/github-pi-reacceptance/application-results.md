# 独立应用实际操作

首候选为模型实际提交并推送的 `33b12109b69c8a9b4ec6c15c602c4e6c51c6d388`。主 Agent 从固定 Git archive 运行应用，不修改业务代码；原始 17 次 CLI 操作、参数、退出码、stdout/stderr 与操作前后存储 SHA 位于 `/Volumes/WorkSSD/Development/braid-gh-pi-reacceptance/independent-app/raw/first-candidate-operations.json`。这是生成应用验收，不是 Braid/Factory 设施测试。

正常创建三条记录取得不同 ID，跨进程 list 保持这三次创建顺序；中文、空格、引号、换行保存在正文中。archive 后默认列表排除该条，list --all 显示其 archived=true，原正文保留；第二次 archive 返回 already-archived 且存储字节不变。无效 ID 返回6，不存在的合法 ID 返回7，均未改数据。损坏 JSON 的 list/add/archive 均返回4及具体行错误，错误根结构的三种命令均返回5及 root not a JSON object，原始数据 SHA 完全不变。

创建顺序边界未满足。另建合法 NDJSON 数据副本，保留上述三个实际记录的物理顺序、ID和正文，仅将 created_at 依次设为3000、2000、1000；list --all 实际倒序返回，原文件未改。源码 cmd_list 的 (created_at,id) 排序及 README 的时钟回拨限制与观察一致。这没有修改宿主时钟，不声称实际发生过系统时钟回拨；它证明应用以时间戳排序替代存储创建顺序，需按冻结需求及公开澄清修正。本候选不能整体判通过，原件保留，已由运行 Owner 通过正常业务协作反馈。

后续：等待模型接受关联 Issue 变更及修正后的固定候选，独立核对重复标题、list --all、创建顺序边界和既有拒写/幂等行为。

修正候选 `95fa210c8e5692be1af8c89fc8e2f128ff1a8911` 的顺序 16 次实际 CLI 操作完成于用户要求降载后，用时约0.43秒，没有模型调用或并发。原合法时间戳逆序副本现保持物理记录顺序；同标题新增两条返回唯一ID，默认list和list --all分别显示未归档与完整归档状态；中文换行正文持久化、重复归档字节不变、无效/不存在ID错误6/7，以及损坏JSON/坏根list/add/archive错误4/5和原数据字节不变全部观察通过。原始操作位于 `independent-app/raw/second-candidate-operations.json`。第二候选应用要求可采用，首候选失败及模型/tmp纪律偏差不抵销。
