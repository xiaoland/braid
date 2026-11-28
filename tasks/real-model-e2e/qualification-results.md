# 独立真实产物与输入观察

所有观察来自真实Braid Agent产物、原生输入或生成应用公开CLI，不运行Braid测试或模拟。原始文件在 `/Volumes/WorkSSD/Development/braid-e2e/qualification`；模型结果、候选发布和原生终态分别判断。

内建Issue首个实际user输入可见标记存在、HTML隐藏标记不存在。PR首个实际user输入包含关联Issue可见标记、不包含HTML隐藏标记，且设计变更前没有list --all；仅定向读取user输入，未展开assistant/CoT。元数据快照中provider均为minimax、model均为MiniMax-M3；原生cost=0不据此声称免费或账户实际扣费。

Issue Agent引导提交 `e2ae347be95a9d49f015b5c33cb36bf6d4dee553` 与其父 `2d65e72bde70d4ddb006f4b3f0ead030a01bb730` 的tree均为 `a7dde826386b8cb7bcc54152f8a24b5f590ed0ba`，diff为空，没有预制业务实现。

PR Agent真实实现固定提交 `25f92ed815c2d268b07da2ed72b8bc567c895ddd` 仅增加notes.py和更新本轮README，未越过业务目录。该提交自publish被gateway清洁判定拒绝，尚未发布；下列结果只证明此Git产物符合原始应用需求，不能证明黄金路径或发布成功。

从固定OID提取原件后，独立实际运行8次CLI：中文/引号新增、第二条新增、另进程持久读回、归档、默认列表排除归档、重复归档、无效ID、损坏JSON。正常操作退出0，内容和创建顺序保持；重复归档没有修改存储。无效ID退出4并显示具体ID，损坏JSON退出3并显示位置与解析原因，两者原存储SHA前后一致。归档保留原正文。原始命令、stdout/stderr、退出码及存储在 `builtin-initial/observations.json`。list --all及重复标题为后续设计变更，当前未以此候选验收。

修复版全新内建attempt2的Issue首user record `7dfbe09f`（08:23:10.559Z，1127字符）与PR首user record `a49e7953`（08:25:09.191Z，4693字符）均含可见标记、不含HTML隐藏标记。PR首输入没有尚未提出的list --all设计。只核对首条user Context，不以此推断全部工具读取；证据为builtin-attempt2-first-context.json及builtin-attempt2-first-pr-context.json。

内建attempt2首发布候选 `1af808614e0112274ac7eb0baff3d1839599264e` 由真实PR Agent自行publish。根提取固定OID原件，独立8项原定CLI操作符合要求：正常退出0，invalidID退出3、malformedJSON退出4且错误具体，两者及重复归档存储SHA不变。另以有效JSON但错误根结构 `[]` 实际add，出现exit1与AttributeError（notes.py line64错误消息对list调用data.get）；文件未覆盖，但错误处理不足。已交旅程Owner通过正常验收讨论让真实Agent修复，未代写应用或采纳。原件、9次实际命令及输出见builtin-attempt2-initial/observations.json。

内建attempt2独立Git读回：引导提交 `ae8c303c4519994eeac67da6b4036fe2c8f1f2df` 与父 `fb1449e3321faa6494472f42a7e6fe2b09aebc37` 的tree均为 `e4d101db478db0151d79d595d62d69a352724036`，diff为空。首发布OID1af8086只更新本轮README并新增notes.py，未跨业务目录，证据builtin-attempt2-published-scope.json。

更新候选 `d639ff30d6a079fb5ede8dbd8a884feeaca3a4c0` 由PR Agent实际publish，根固定OID独立10次CLI操作通过：同中文引号标题两个不同ID，跨进程创建顺序保持，归档正文保留且默认排除，list --all按顺序标注archived/active，重复归档SHA不变，无效ID具体错误exit3，损坏JSON与根结构 [] 具体错误exit4、无traceback且原文件SHA不变。原件和命令输出在builtin-attempt2-design/observations.json。业务候选可正常采纳，等待修复Console Diff身份契约实际部署确认。

Mac存储合规失败单独记录：runtime Owner定向tool arguments确认29个包含/tmp的实际调用，涉及/tmp/issue1-design.md、/tmp/pr1-body.md及/tmp/notes-evidence/*；原始证据builtin-attempt2/raw/system-tmp-tool-facts.json。已通过正常人类讨论明确未来路径必须WorkSSD并禁止/tmp，未代改应用、未擅自删除旧现场；业务操作通过不抵销此项失败。

设计更新实际送达方式是普通wake的Event References，随后Agent主动读公开object show。定向toolResult `3fffb0dc`（4525字符）含list --all与[]反馈，也含HTML隐藏标记；不能把首user canonical Context过滤扩大为所有工具输入过滤。该入口返回原始对象，Context合同由projector持有。advisor发现系统提示把canonical state重读指向show，已交Owner改用既有object context，不改变原始对象API或声称HTML安全隔离。词标记证据builtin-attempt2-design-tool-context.json。

新版提示后的真实PR g2会话已实际调用object context 2，指定toolResult `92700617`（6077字符）由根独立词标记核对：可见需求在、list --all设计在、HTML隐藏标记不在。这证明现有canonical重读路径实际生效，不追认旧rawshow过滤；证据builtin-attempt2-new-guidance-context.json。

正常操作者采纳后，根独立Git读回main为 `77c8f50a5323756dc85d7a8e7f2dde2d91526c6f`，两父提交分别为基线fb1449e与已验收d639ff30；merge与候选tree均为 `a9f1fc78c702eaa33cd2b4cbd052b74286a0631f`，diff为空，证明真正采纳该候选，无需重复同内容应用CLI。证据builtin-attempt2-merge.json。

内建原生元数据完整收口：attempt1 83条、attempt2 205条assistant记录均为minimax/MiniMax-M3，记录id+timestamp无重复；只取usage/model元数据，未展开正文或CoT。两次输入合计346323、输出102800、缓存读取6135240 token（reasoning已包含输出，不另加）。按本轮准备记录2.10/8.40/0.42 CNY/M估算4.167599元；这是准备价格下的估算，账户扣费未知，原生cost=0不代表免费。证据builtin-usage-summary.json及两次model-metadata-final.json。

GitHubattempt2首真实Issue37 user record `f4799623`（2732字符）由根定向核对：可见标记在、HTML隐藏标记不在、WorkSSD约束在，尚未提出的list --all不在。原生Issue turn01a12a40-9489-7742-8359-537e7b1b4933实际运行；证据github-attempt2-first-context.json。

GitHubPR38首实际user record `0b96c74d`（5342字符）含关联可见需求和WorkSSD约束，不含HTML隐藏标记，也无尚未提出的list --all。Issue Agent自主ensure引导commit `8cf6782a4792d5e571d9aa708d43cacc11b6362c` 与父基线 `2cc02e78920072fdf1165def8798464012630655` tree均为 `c7d6db7a9f04cc80e4f0fa0828ae7de8005ab5dc`，diff为空；无预制业务实现。独立证据github-attempt2-first-pr-context.json和github-attempt2-bootstrap.json。

GitHub首发布业务候选 `e08485391b252892fec16f6678fb6084e0de9dbf` 固定原件独立9次CLI符合原定8项，另有效JSON但错误结构[]具体拒绝、无traceback、不覆盖原文件。Unicode/引号/换行正文保持、不同ID、跨进程顺序、归档保留正文、默认排除、重复归档字节不变，无效ID与损坏JSON均具体非零且原数据保持。证据github-attempt2-initial/observations.json。候选提交/推送早于定向观察到的跨空间工具操作，但执行后续实际读旧builtin配置并SQLite修改旧账本后恢复；不能把候选业务合格写成全程隔离或合规通过。

GitHub设计更新后的新PR原生会话09:37:03首user record `fd3e888a`（09:38:20.204Z，10381字符）由根定向核对：可见标记、list --all与WorkSSD约束在，HTML隐藏标记不在。证据github-attempt2-design-context.json；只证明该Context输入，不扩大到任意工具读取。设计正文编辑即时PR已completed，本次只计idle更新，活动更新另行取证。

活动正文更新实际interrupt旧turn并重建会话后，首user `053cfb4d`（09:39:38.377Z，10969字符）含可见需求、list --all、active/archived和WorkSSD，不含HTML隐藏标记。证据github-attempt2-active-design-context.json；活动状态与reset因果来自Owner原件active-body-reset-observed.json，区别于此前idle正文更新。

GitHub设计候选 `08c2458751a02a8143e0cb7fe620c20bd48dea16` 官方head readback后固定原件，根独立10次真实CLI均符合：同中文引号标题不同ID，跨进程创建顺序与正文保持；archive默认排除，list --all按原顺序标明archived/active；重复归档字节不变；invalidID、brokenJSON与[]具体非零且原数据不覆盖，[]无traceback。业务改动仅本轮目录README/notes.py，证据github-attempt2-design/source.json及observations.json。可按批准旅程正常采纳，网络补齐/关闭重开/合并与终态另行取证。

正常GitHub合并后，根读取实际fetch的origin/main `8281fffa0bb76398a0cd4c262bd69351e9ed2da6`，两父提交为blank需求base2cc02e7与qualified08c245，merge与候选tree均为 `d84062e916f597a402cfe8d292d3a0945a1c9421`，diff为空。证据github-attempt2-merge.json，证明实际采纳验收内容，不重复同内容应用CLI；Harness生命周期和最终停止另看Owner事实。
