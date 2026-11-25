<p align="center">
  <img src="assets/branding/app-icon.png" width="96" alt="Wisp Study 的应用图标">
</p>

<h1 align="center">Wisp Study</h1>

<p align="center">
  <a href="README.md">English</a> · <b>简体中文</b>
</p>

<p align="center">
  <b>本地优先的文献库：收集、筛选、阅读，向你的文献提问。</b><br>
  用 Rust + GPUI 原生绘制。文献和 PDF 留在你的电脑上，每个文献库就是一个文件夹。
</p>

<p align="center">
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS-lightgrey" alt="Windows 和 macOS">
  <img src="https://img.shields.io/badge/built%20with-Rust%20%2B%20GPUI-dea584?logo=rust" alt="用 Rust 和 GPUI 构建">
  <a href="LICENSE"><img src="https://img.shields.io/github/license/xuzhougeng/WispStudy?color=green" alt="MIT 许可"></a>
  <img src="https://img.shields.io/badge/status-alpha-orange" alt="Alpha">
</p>

<p align="center">
  <a href="https://xuzhougeng.github.io/WispStudy/"><b>主页</b></a> ·
  <a href="#安装"><b>安装</b></a> ·
  <a href="#功能">功能</a> ·
  <a href="#常见问题">常见问题</a> ·
  <a href="https://xuzhougeng.github.io/WispStudy/tutorials/">教程</a> ·
  <a href="docs/ZOTERO.md">Zotero 导入</a> ·
  <a href="docs/CLI.md">命令行与 MCP</a> ·
  <a href="docs/DATA.md">数据与隐私</a> ·
  <a href="docs/DEVELOPMENT.md">开发</a>
</p>

<p align="center">
  <img src="docs/images/library.png" width="100%" alt="Wisp Study 的文献库页面：左侧是视图和标签，中间是文献列表，右侧是所选文献的详情和笔记。">
</p>

> [!NOTE]
> Wisp Study 还在早期，变化很快。支持中文和英文界面，在「设置 › 通用 › 界面语言」中切换（Settings › General › Interface language），立即生效并自动保存。第一次用，可以跟着[教程](https://xuzhougeng.github.io/WispStudy/tutorials/)从零建一个文献库；之后的几篇各讲一个科研场景：精读、提问、提取、写综述、写论文、改稿。遇到问题或有想法，欢迎[提 issue](https://github.com/xuzhougeng/WispStudy/issues)，更欢迎直接发 pull request。

## 为什么是 Wisp Study

**🔒 本地优先，文献是你自己的。** 不需要账号，没有云端。每个文献库是数据目录下的一个独立文件夹，数据库、PDF、自定义词条都在里面，可以单独备份或整个拷走。删除只是移动到 `removed/`，什么都不会真的丢。哪些数据会离开本机，逐条写在[数据与隐私](docs/DATA.md)里。

**🐋 先广泛导入，再让助手帮你筛。** 为 PubMed、Europe PMC、Crossref、arXiv 保存检索式，成批导入；助手逐篇阅读，给出建议、理由和一句逐字引自原文的依据。建议只是建议：每一篇收不收，由你确认。

**💬 回答带出处，出处能点开。** 向整个文献库或单篇文献提问，材料不够时助手自己去检索全文、按页翻阅。回答里的 `[P3 p.5]` 可以点击，打开 PDF 跳到那一页，所引的原句在页面上高亮。

**⚡ 原生界面，不是网页。** 整个界面由 Rust 和 [GPUI](https://www.gpui.rs/) 在 GPU 上绘制，没有 WebView，没有 Electron。PDF 用纯 Rust 的 [hayro](https://github.com/LaurenzV/hayro) 渲染，不依赖系统 PDF 库。

## 安装

从[发布页](https://github.com/xuzhougeng/WispStudy/releases/latest)下载：

| 平台 | 文件 | 说明 |
|---|---|---|
| **macOS**，Apple 芯片 | `WispStudy-v<版本>-macos-arm64.dmg` | Developer ID 签名并经 Apple 公证。打开后把 Wisp Study 拖进「应用程序」 |
| **macOS**，Intel | `WispStudy-v<版本>-macos-x64.dmg` | 同上 |
| **Windows** 10 / 11（x64），安装版 | `WispStudy-v<版本>-windows-x64-setup.exe` | 装在当前用户名下（`%LOCALAPPDATA%\Programs\Wisp Study`），不需要管理员权限，带 Authenticode 签名 |
| **Windows** 10 / 11（x64），便携版 | `WispStudy-v<版本>-windows-x64.zip` | 解压后运行 `WispStudy.exe`，不写注册表，带 Authenticode 签名 |
| **Linux**（x64） | `WispStudy-v<版本>-linux-x64.tar.gz` | 解压后运行 `wisp-study`；没有签名，也没有在真机上测试过 |

Wisp Study 原名 Wisp Paper。旧版的自动更新找不到改名后的版本，请从发布页下载新版安装：Windows 安装版会原地替换旧版；macOS 装好 `Wisp Study.app` 后可删去旧的 `Wisp Paper.app`。文献库和设置原地保留，不用迁移。

命令行工具 `wisp` 随包附带：Windows 和 Linux 在主程序旁边，macOS 在 `Wisp Study.app/Contents/MacOS/wisp`。

从源码构建需要 Rust（版本见 `rust-toolchain.toml`，rustup 会自动安装）；Windows 上还需要 MSVC 构建工具。

```sh
git clone https://github.com/xuzhougeng/WispStudy
cd WispStudy
cargo run --release
```

想先看看效果，可以生成一个带示例数据的目录，不碰你真正的文献库：

```sh
cargo run -p wisp_core --example seed -- /tmp/wisp-demo some.pdf
WISP_STUDY_HOME=/tmp/wisp-demo cargo run
```

### 在 Windows 上编译 exe

exe 要在 Windows 上编译，macOS 和 Linux 上交叉编译不出来。先装好两样（只需一次）：

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive"
winget install Rustlang.Rustup
```

第一条装的是 MSVC 构建工具（「使用 C++ 的桌面开发」，含 Windows SDK）。在仓库里第一次运行 `cargo` 时，rustup 会装上 `rust-toolchain.toml` 指定的版本。

只要 exe，在仓库根目录运行：

```powershell
cargo build --release -p wisp_study -p wisp_cli
```

产物是 `target\release\wisp_study.exe`（应用，不带控制台窗口）和 `target\release\wisp.exe`（命令行，`wisp mcp` 是 MCP 服务）。图标和 `assets/` 已编进 exe，不用另外带文件。

要打成和发布页一样的包，运行 `scripts/package-windows.ps1`：

```powershell
pwsh -File scripts/package-windows.ps1 -NoInstaller   # 只出便携版 dist\WispStudy-v<版本>-windows-x64.zip
winget install JRSoftware.InnoSetup                   # 要安装版时先装 Inno Setup 6
pwsh -File scripts/package-windows.ps1                # 便携版加 dist\WispStudy-v<版本>-windows-x64-setup.exe
```

`-Sign` 用 SSL.com eSigner 签名，要先设好 `ESIGNER_*` 变量，平时用在发布流程里，本地试用不需要。手动试运行前可以先设 `$env:WISP_STUDY_HOME="C:\tmp\wisp-test"`，这样不会碰到 `Documents\WispStudy` 里的文献库。

提问、建库向导、筛选、翻译和写作页的模型分节需要模型服务，在「设置 › AI」里配置：Claude API、OpenAI 兼容的 chat/completions，或 OpenAI 的 Responses 接口。可以放进多个模型，提问、批量筛选、翻译、整理全文各用哪一个、各自思考到什么程度分别指定。

## 功能

- **文献库**：多个独立的文献库，各有名称、主题描述和标识色。
- **收集**：粘贴 DOI 或 PMID，拖入 PDF，导入 BibTeX / RIS，或手动录入；也可以[从 Zotero 本地数据库导入](docs/ZOTERO.md)文献信息、标签、笔记及已下载的 PDF。按 DOI、PMID、题目去重。成批导入有进度和结果汇总。
- **整理**：搜索、筛选、排序；阅读状态、收录判断、收藏、标签、笔记；每个库自己定义的词条；回收站；导出 BibTeX 和 CSV。
- **笔记保存**：「我的笔记」可保存到已登录的 Notion MCP，或导出为 Markdown 和图片附件。问答页可选择历史问题，预览后保存；原笔记和对话保留。助手也能调用相同工具，见 [笔记导出](docs/CLI.md#笔记保存与导出)。
- **检索与导入**：保存检索式，先试检索再导入，每次导入可以整批撤销；可以定时自动更新。
- **建库向导**：写下主题，助手为各个数据库拟几条检索式并逐条试检索，你挑靠谱的。
- **筛选**：助手几篇同时读，给出收录建议和原文依据，你来确认；也可以按建议整组采纳。
- **翻译**：题目和摘要一键译成中文，译过的保存在文献库里。
- **写作**：分部分写稿（摘要、导言、方法、结果、讨论），旁边列出库里文献在同一部分的句子：还没动笔时是各篇的开头，写起来之后是与正在写的这一段最相近的句子，中文也能找。句子可以插入引用、复制，或在 PDF 里定位到原句。入库的 PDF 按版面读出文字、按标题分节；没有标题可依的（如 Nature 的导言）可以让模型判断，扫描件可以让模型读页面。稿件可以从零写，也可以导入已有的 Word / Markdown 文件，或接着改助手写的综述；助手像 Word 的审阅那样留批注、提修改，由你采纳；导出的 Word 写回原文件，带批注，改动可以记为修订。写作有两种方式，在大纲上方切换：「自己写」是你动笔、助手在旁边帮；「助手写」是左边大纲、中间和助手对话、右边稿件，由助手查文献、动笔，你看着它写、随时亲手改，每一节都能翻回早先的版本。
- **提问**：对文献库或单篇文献提问，回答按 Markdown 排版；助手引的原句会和原文逐字比对，结果标在出处上，点击出处在 PDF 里高亮。单篇提问时对话和 PDF 并排，PDF 上选中的原文或框出的截图可以直接带进问题；回答里的文字可以选中、复制、引用。输入 `/` 选择命令（检索全文、配引用、分阶段写综述、比较文献、逐条核对），可以粘贴图片，对话保存为历史、可以接着问，回答可以存为这篇文献的笔记。
- **PDF**：在详情面板里直接看，面板宽度可以拖动；缩放、适应宽度或整页；选字、复制、截图；下划线、高亮和附加笔记。标注作为独立 metadata 保存并叠加显示，助手也能读取原文、笔记和位置。读不出文字的 PDF 会说明原因，可以让能看图的模型读页面。
- **图片库**：在 PDF 上框出一张图或一个表，收藏进「收藏」下的图片库；图号和图注从页面上读出来，也可以让模型看图写图注、标签和解读。按标签和关键词找图，一键回到原文的那一页。
- **查找开放全文**：为没有 PDF 的文献批量查找开放获取的全文，下载后附到原条目；不绕过付费墙。
- **命令行与 MCP**：`wisp` 不开窗口就能新建文献库，导入本地 PDF / 文件夹、BibTeX / RIS 和 Zotero，并读写同一个数据目录；`wisp mcp` 把同一组工具交给外部的 AI 客户端。见 [docs/CLI.md](docs/CLI.md) 和 [SKILL.md](SKILL.md)。
- **设置**：浅色 / 深色，可改键的快捷键，全局代理和每个模型服务单独的代理。改动即时保存。

每一项的细节在 [docs/FEATURES.md](docs/FEATURES.md)。

## 常见问题

### 一篇文献可以放在多个文献库里吗？在 A 库记的笔记，B 库里看得到吗？

可以放，但笔记看不到。每个文献库是一个独立的文件夹，去重只在库内做，所以同一篇可以分别导入 A 库和 B 库；两边是各自的一条记录，笔记、标注、标签、阅读状态和收录判断都只属于所在的库，不会同步。

这是有意的。一个文献库对应一个主题，同一篇文献在不同的主题下读法不一样：在 A 库为它记的笔记，到了 B 库未必用得上；B 库里可能连全文都不需要，有摘要就够了。所以多数时候，一篇文献放在一个库里就好。

拿不准怎么分，就从一个库开始。比如建一个专门读文献、学东西的「学习」库，里面的主题可以很杂，用标签区分。以后哪个方向长成了独立的课题，再建新库，用文献右键菜单里的「移到文献库…」把相关的文献搬过去，PDF、标注和关于它的对话一起走。

### 一个文献库里为什么不能再分组（文件夹、分类）？

文献库本身就是分组，库里面用标签。

文件夹要求一篇文献只待在一个地方，而文献很难被这样干净地分开：一篇用单细胞测序研究根毛发育的论文，该放进「单细胞」还是「根毛」？丑小鸭定理（渡边慧，1969）说的就是这件事：没有先验假设时，任何两个对象在理论上一样相似，怎么分类完全取决于主观上看重哪些特征。

标签不要求唯一的归属。一篇文献可以带几个标签；侧栏里按住 `⌘` / `Ctrl` 多选标签，只留同时带着它们的文献，按住 `⌥` / `Alt` 点一个标签则是排除它。从 Zotero 导入时，分类路径也转成标签。

### 两个文献库之间能关联起来吗？

不能，这也是有意的。从设计之初，一个文献库就是一个独立的个体，有自己的主题、收录标准、标签和自定义词条。两个库的目标不同，最好互不相干，避免交叉污染。

需要横跨几个库时，交给首页的助手：它读得到书架上所有的文献库。直接说「比较一下『A』和『B』两个库里关于……的研究」，它会到各个库里列文献、检索全文、读原文，回答的每条出处写明来自哪个库，点一下就打开那个库里的那一篇。别的库在这里只读，它不会改动其中任何一个。

### 文献库太大，提问会不会很费 token、答得不准？库该怎么建？

一次提问的消耗看的是当前列表，不是整个库，而且有上限。向文献库提问时，范围是列表里显示的那些文献（受视图、标签、搜索和筛选影响）：与问题最相关的给完整记录（题目、摘要、笔记），放不下的只给题目，全文只取与问题最相关的几十页。所以消耗不会随着库无限增长，不过长列表确实比短列表贵。材料不够时，助手再自己到库里检索全文、按页去读。

省 token 和答得准是同一件事：先缩小范围，再问。

- 点一个标签、换一个视图或搜一个词，然后提问，模型拿到的就只是这一部分文献。
- 只关心一篇，就在这篇文献的「提问」里问，模型读的是它的全文。
- 列表很长时，问题里带上文献里会出现的词（英文文献就带英文术语）：哪些文献给完整记录，是按这些词在题目、摘要、标签和笔记里出现得多不多来排的。
- 想核对，展开对话顶部的「文献」：每篇交给模型的是完整记录还是只有题目、给了几页全文，都写在那里。回答里的出处可以点开，助手引的原句会和原文逐字比对。用 DeepSeek 的模型时，回答下面还有这次用了多少 token、约合多少钱。

建库的几条建议：

- **一个库一个目标**：一个课题、一篇综述、一个要学的方向。分不分库，看目标和收录标准是不是不同，不看篇数。
- **写好主题描述**。助手拟检索式、筛选和回答时都会读到它。
- **先广泛导入，再筛选**。同一个筛选目标下每篇只读一次，不相关的移进回收站，留下的才是以后每次提问的范围。库干净比库小要紧。
- **批量的活交给便宜的模型**。筛选、翻译、整理全文各用哪个模型，在「设置 › AI」里分别指定，提问再用强的。

## 文档

| | |
|---|---|
| [教程](https://xuzhougeng.github.io/WispStudy/tutorials/) | 十八篇按科研场景写的教程，带截图：建库、先聊后建库、迁移、只读一个 PDF、精读、思维导图、对照着读、听论文、组会幻灯片、提问、按影响因子和分区挑文献、提取、写综述、写论文、直接写与在 Wisp Study 里写、改稿、跟进新文献、几个课题一起推进 |
| [建库案例](docs/TUTORIAL.md) | 从零建一个「植物单细胞转录组」文献库的原始记录（v0.2.0） |
| [功能详解](docs/FEATURES.md) | 每个页面能做什么 |
| [数据与隐私](docs/DATA.md) | 文件放在哪里，哪些数据会离开本机 |
| [命令行与 MCP](docs/CLI.md) | `wisp` 的用法、工具一览、MCP 配置 |
| [智能体技能说明](SKILL.md) | 供外部智能体使用的 `SKILL.md`：支持范围、建库与导入流程、命令示例 |
| [开发](docs/DEVELOPMENT.md) | 构建、测试、代码结构 |

## 致谢

文献部分移植自 [BiblioCell](https://github.com/xuzhougeng/BiblioCell)（它的数据集和网页版没有移植），应用结构和回答里的 Markdown 读法参考 [quiet-md](https://github.com/s1dashu/quiet-md)。

## 许可

代码以 [MIT](LICENSE) 许可发布。随附的第三方代码和素材见 [THIRD_PARTY.md](THIRD_PARTY.md)。
