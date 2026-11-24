# 命令行与 MCP

`wisp` 是不开窗口的入口，读写和应用同一个数据目录，应用开着时也能用。它提供的每个工具既是一个子命令，也是一个 MCP 工具，应用内的助手用的也是同一张工具表。

供外部智能体使用的完整操作说明见 [SKILL.md](../SKILL.md)，包括能力边界、PowerShell 示例、导入结果核对和 MCP 配置。

## 建库与导入

当前 CLI / MCP 可以向**已有文献库**导入在线检索结果，流程是 `search_online` 试检索、`save_search` 保存检索式、`run_search` 执行导入；PDF 可再用 `find_pdf` 获取开放版本。

也可以完全通过 CLI 从零建库并导入本地资料：

```sh
wisp create_library --name "植物单细胞" --topic "细胞图谱与发育轨迹" --color green
# 用上一步返回的真实 id 替换 L-实际ID
wisp import_references -l L-实际ID --path references.bib
wisp import_references -l L-实际ID --path references.ris
wisp import_pdf -l L-实际ID --path paper.pdf
wisp import_pdf -l L-实际ID --path "PDF collection"
wisp import_zotero -l L-实际ID --path /path/to/Zotero/zotero.sqlite
wisp list_zotero_libraries --path /path/to/Zotero/zotero.sqlite
wisp import_zotero -l L-实际ID --path /path/to/Zotero/zotero.sqlite --zotero-library "Root Lab"
wisp import_zotero -l L-实际ID --path /path/to/Zotero/zotero.sqlite --zotero-collections "植物/根"
```

`import_pdf` 支持递归导入文件夹，子文件夹名称转为标签；`--paper-id` 可将单个 PDF 附到已有文献。离线提取 DOI 和全文，未补全的题目用文件名，不自动联网查询元数据。已有记录保留，回收站条目跳过，相同条目的相同附件内容只保存一次；内容完全相同的文件换了名字再导入，算作已持有它的那篇文献，不新建记录。

`import_references` 按内容识别 BibTeX / RIS，不自动携带 PDF。`import_zotero` 复用桌面的只读迁移流程，复制可访问的本地 PDF；默认导入个人库和所有群组库，`--zotero-library` 只导入其中一个（「我的文库」、群组名称，或 `list_zotero_libraries` 列出的 `id`；名称对不上时报错并列出有哪些），`--zotero-collections` 只导入其中几个分类（分类路径如 `植物/根`、名称或分类 `id`，几个时写 JSON 数组；每个含其子分类）。建议先退出 Zotero，详见 [Zotero 导入](ZOTERO.md)。这些本地导入不联网、不移动源文件。

PDF / Zotero 的 JSON 含新增、跳过、附件数量和逐项 `results`。必须检查 `failed` 和 `results[].problems`：有些记录已入库，但附件可能缺失，部分失败仍可能退出 0。PDF 的 `textless` / `textError` 表示文件已保存但未提取出文字。Zotero 附件的文本在阅读时提取。本地导入不生成在线检索的批次号，不能用 `undo_import` 撤销。

`--data-dir` 只选择数据根目录，建库用 `create_library`；`save_search --name` 命名的是检索式。直接按 DOI / PMID 添加条目仍使用桌面入口。旧发布包缺少新命令时需更新或从源码构建。

## 命令行

### 笔记保存与导出

桌面「我的笔记」标题右侧的「保存 / 导出」打开保存窗口，选择 Notion 或本地文件后保存；已保存的页面通过同一标题栏上的 Notion 链接打开。问答页的同一入口可勾选历史问题，选后自动更新导出内容。窗口可修改本次保存的标题、展开内容预览；正文直接使用原笔记或所选问答。原文摘录、回答原有链接及提问图片会随笔记保留。

助手、CLI 和 MCP 共用这些工具：

| 工具 | 用途 |
|---|---|
| `list_note_sources` | 查询对话 ID、从 1 开始的问题序号、已保存及待确认记录 |
| `prepare_note` | 预览个人笔记与选定问答的 Markdown、图片文件名 |
| `export_note` | 写入用户指定的 `.md` 文件，并复制图片到相邻附件目录；不覆盖已有文件 |
| `save_note_to_notion` | 使用已启用并登录的官方 Notion MCP，保存到用户指定的父页面 |
| `resolve_note_save` | 用户已检查 Notion 后，确认上一次写入成功或完全未写入 |

省略 `paperId` 表示文献库问答；填写时默认包含「我的笔记」，只导出问答可设 `includeNotes: false`。`conversations` 为 `[{ "id": "实际对话ID", "questions": [1, 3] }]`，省略问题序号表示该对话全部问题。`title` 和 `markdown` 可传入编辑后的草稿；其中图片文件名沿用预览结果。

```sh
wisp list_note_sources -l L-实际ID --paper-id P-实际ID
wisp prepare_note -l L-实际ID --paper-id P-实际ID
wisp export_note -l L-实际ID --paper-id P-实际ID --path "/existing/folder/note.md"
wisp save_note_to_notion -l L-实际ID --paper-id P-实际ID --parent "用户指定的Notion父页面链接"
```

Notion 首次保存创建子页面，同一文献及父页面的后续保存只在末尾追加；文献库问答单独一页。完全相同的草稿会跳过，修改后的草稿作为新版本追加，重叠选择的不同批次目前不会逐题去重。结果不确定时不会自动重试；先核对远端内容，再通过窗口或 `resolve_note_save` 处理待确认记录。部分写入应在 Notion 整理后确认，不能标记为完全未写入。运行中的保存不能被其他进程清除记录。

这一阶段使用现有年份，不引入完整出版日期。自动建立文献库目录、可配置标题模板与元数据顺序、问题提炼不在本轮范围。保存按钮不调用模型；Agent 模式会遵循现有写入确认流程。`wisp mcp --read-only` 不提供导出、Notion 写入或保存记录核对工具。

```sh
cargo build --release -p wisp_cli       # 生成 target/release/wisp

wisp help                               # 命令一览
wisp list_libraries
wisp list_papers -l 大语言模型 --query attention --limit 10
wisp get_paper <文献 ID>
wisp search_fulltext "10x Genomics Chromium"
wisp read_fulltext <文献 ID> --page 3
wisp list_pdf_annotations --paper-id <文献 ID> --query "生长"
wisp search_fulltext --help             # 一个工具的参数
```

输出是 JSON；出错时信息写到 stderr，退出码为 1。只有一个文献库时可以省略 `-l`；`--data-dir 文件夹` 指定另一个数据目录。

## 工具

| 工具 | 用途 |
|---|---|
| `list_libraries` | 所有文献库的 ID、名称、主题和文献数量；首页上还没有建成文献库的对话不列出，也不算在「只有一个文献库时可以省略 `-l`」之内，用 ID 指定时可以访问 |
| `list_pdf_annotations` | 只读查询 PDF 下划线、高亮、方框、便签、截图标记（`cut`）及笔记；可按 `--paper-id`、`--resource-id`、`--page`、`--kind`、`--query` 筛选，用 `--limit`、`--offset` 分页。返回 `quote`（原文）、`note`（用户笔记）、1-based 页码、页面左上角起的点坐标 `boxes`、附件 ID 和时间；`get_paper` 的 `pdfAnnotationCount` 给出总数。内置助手与只读 MCP 也可使用 |
| `create_library` | 新建文献库；`--name` 必填，`--topic`、`--color` 可选，返回新库 ID（会创建文件和数据库） |
| `export_library` | 把整个文献库导出为 `.wispstudy` 文件，`--path` 必填，已有文件不覆盖；`--attachments false` 不带 PDF 等附件 |
| `import_library` | 把另一个 Wisp Study 导出的 `.wispstudy` 文件（或改名前的 `.wisppaper` 文件）导入为新文献库，`--path` 必填（会创建文件和数据库） |
| `import_pdf` | 复制本地 PDF 或递归导入文件夹，`--path` 必填；单文件可用 `--paper-id` 附到已有条目（会改动文献库） |
| `import_references` | 导入本地 BibTeX / RIS 文件，`--path` 必填，按内容识别格式（会改动文献库） |
| `import_zotero` | 从本机 `zotero.sqlite` 迁移文献、笔记、标签和 PDF，`--path` 必填；`--zotero-library` 只导入其中一个文献库，`--zotero-collections` 只导入其中几个分类（会改动目标文献库） |
| `list_zotero_libraries` | 列出本机 `zotero.sqlite` 里的个人库和群组库，各有 `id`、名称和会导入的条目数；以及各文献库的分类（`collections`：`id`、`path`、所属文献库 `library` 和含子分类的条目数），`--path` 必填（只读） |
| `library_status` | 一个库的概况：各状态的数量、自定义词条、状态值的显示名称 |
| `list_papers` | 搜索、筛选、翻页；规则与界面上的列表相同。年份可以是区间（`--year-from 2020 --year-to 2026`，只给一头也可以），期刊可以按影响因子（`--min-impact 10`）和分区（`--zone cas:2` 是中科院 1–2 区，`xr:1` 新锐 1 区，`jcr:1` JCR Q1）筛 |
| `mind_map` | 一篇文献的思维导图：大纲（Markdown 嵌套列表），节点后面方括号里是出处的页码和原句；没有导图时 `nodes` 为 0 |
| `get_paper` | 一篇文献的完整记录、词条和附件；`journalMetrics` 是它的期刊的影响因子与分区（查不到为 null） |
| `journal_metrics` | 查一种期刊的影响因子与分区（JCR 2025、中科院分区表升级版 2025、新锐期刊分区表 2026、国际期刊预警名单 2025）：`--journal` 给刊名（全称或缩写）或 ISSN，或 `--paper-id` 用一篇文献的期刊；数据随应用附带，不联网 |
| `list_figures` | 图片库：从 PDF 里截下来收藏的图片、自己上传的图片（`uploaded`）和模型生成的图片（`drawn`），各自的图号、图注、标签、备注、出处（文献和页码）、图片文件的路径，以及它在稿件里的地址（`address`：稿件里单独一行写 `![图注](address)` 就放进这张图）；可按 `--query`、`--tag`、`--paper` 筛选 |
| `search_fulltext` | 在已保存的全文里找短语或关键词，返回页码和逐字片段 |
| `read_fulltext` | 按页读取全文，一次最多 8 页 |
| `find_citations` | 为一段文字逐句找出库里可能支持它的文献和它自己最贴近的一句原文（按词语找出的候选；文字和文献语言不同时用 `queries` 给各句配检索词） |
| `find_passages` | 在一篇文献里找出最贴近给定句子的原文：稿件里引用它的那句话依据的是哪一句（摘要和全文里的句子，全文里的带页码；语言不同时用 `queries` 给出检索词） |
| `find_sentences` | 写作参考：库里各篇文献在同一部分（摘要、导言、方法、结果、讨论）的句子；给 `--query` 时按相近程度排，不给时返回各篇这一部分的开头 |
| `search_online` | 在 PubMed、Europe PMC、Crossref 或 arXiv 试运行一个检索式，不写入；标出每条结果是否已在库中 |
| `web_search` | 用网页搜索引擎搜索互联网（DeepSeek、Jina 或 You.com，按桌面「设置 › 网络 › 网页搜索」；`--engine` 指定一个），返回标题、网址、摘要、站点和发布时间；`--content` 附上网页正文，`--count` 默认 8。不需要文献库，不写入。DeepSeek 的 Key 与它的模型共用，Jina、You.com 的 Key 也可以放在环境变量 `JINA_API_KEY`、`YDC_API_KEY` |
| `find_metadata` | 为缺 DOI 或题目的文献按题目到 Crossref、Europe PMC 和 arXiv 查找可能对应的记录，不写入；返回各来源的情况和候选（相似度、一致与不一致之处、是否已是库里的另一篇）。`--paper-id` 必填，`--title`、`--author`、`--year` 可选。写入由人在窗口里核对后确认 |
| `list_searches` / `save_search` / `remove_search` | 查看、保存、删除检索式（后两个会改动文献库） |
| `run_search` | 运行一个保存的检索式并导入，已有的文献跳过，返回批次号（会改动文献库） |
| `list_imports` / `undo_import` | 导入记录；整批撤销一次导入：处理过的文献保留，没动过的移出文献库（不进回收站，以后的检索还能再导入），它们的记录另存为文献库文件夹里的 `removed/undone-import-<批次号>.bib`，路径在结果的 `setAside` 里（撤销会改动文献库） |
| `sync_searches` | 运行到期的检索式（所有开启了自动更新的文献库，或指定的一个）。「自动更新」关着的文献库不会运行，用 `-l` 指定它也一样，结果里 `runs` 为空；`--force` 不看间隔，也不看这个开关，运行所有已启用的检索式（会改动文献库） |
| `suggest_screening` | 为一篇文献提出收录建议（结论、理由、逐字引文、把握）；不改变收录判断，由用户在筛选页确认 |
| `suggest_field` | 为一篇文献的一个自定义词条提出取值（附逐字引文）；不改动词条，由用户在提取页确认。单选词条只接受它的选项 |
| `organise_papers` | 整理已有的文献：`changes` 是一个 JSON 数组，每项给出文献 `id` 和要改的方面（`addTags`、`removeTags`、`starred`、`reading`、`note`）；全部写入或全部不写。不能改收录判断和自定义词条 |
| `list_cite_styles` | 列出导出 Word 时可用的引用样式（CSL）：常用的内置样式、自己添加的和 Zotero 里装的、全部内置样式 |
| `read_cite_style` | 读取一个引用样式的 CSL 定义（`--style csl:nature`）；只读 |
| `try_cite_style` | 用文献库里的文献试排一个样式（`--style` 已有样式，或 `--csl` 一份 CSL 定义），不保存：正文引用和参考文献条目各是什么样子 |
| `read_docx_citations` | 读一个在别处写好的 Word 文件引用了什么（`--path a.docx`），不改文件：正文里的引用、参考文献表的每一条和它是怎样识别的（Zotero 域、文献库里的文献、条目自己的文字…），以及重排时会保持原样的引用和原因；`--lookup` 联网查 Crossref |
| `import_docx_references` | 把一个 Word 文件参考文献表里的文献收进文献库（`--path a.docx`），不改文件：识别方式同 `read_docx_citations`；默认联网查 Crossref（`--lookup=false` 不查），没有 DOI 的条目按文字查、题目和年份相符才补上 DOI。按 DOI、PMID、题目去重，可以重复运行；没能识别的条目列在 `unresolved`，不收进来，`--entries` 给出它们的 CSL-JSON 后只收这几条 |
| `restyle_docx` | 把 Word 文件的引用和参考文献表按一个 CSL 样式重排，写成新文件（`--path a.docx --style csl:nature`，或 `--csl` 一份 CSL 定义；`--output b.docx`，省略时写在原文件旁边、文件名加 `-restyled`；`--track` 把改动写成修订）。引用是能跳到条目的域；原文件不改，已有文件不覆盖 |
| `save_cite_style` | 把一份 CSL 定义存为自己的引用样式（`--name my-journal.csl --csl …`），写在数据文件夹的 `styles/` 下，之后可在写作页「导出」菜单里选用 |
| `import_manuscript` | 把一个在别处写好的稿件文件导入写作页，成为一份新稿件（`--path a.docx`，也可以是 `.md`、`.txt`），等同于写作页的「导入」：原文件不改，正文按标题分节（Word 文件没有用标题样式时按看起来像标题的段落分，结果的 `guessedHeadings` 列出它们），Word 里的批注一起带进来，表格和图片留在文件的副本里。正文里的引用保持文件里的写法。同一个文件已经是稿件时不再导入一份（`already`） |
| `make_slides` | 把内容做成一份 PowerPoint 幻灯片，写成新文件（`wisp make_slides '<slides 的 JSON>' --paper-id P-… --output talk.pptx`）。`slides` 是一个 JSON 数组，每页给 `title`、`bullets`（字符串数组，以两个空格开头的是上一条下面的小点）、`figure`（图片库里一张图的 ID，图注和出处自动带上）或 `image`（本机 PNG / JPEG 的路径）、`caption`、`source`（页脚的出处）、`notes`（讲稿备注）；只有 `title` 的一页是分节页。`--title`、`--subtitle` 是标题页，省略时取 `--paper-id` 那篇文献的题目和作者、年份、期刊。版面由程序排：字号按内容选，要点放不下时接着排到下一页。`--reference lab.pptx` 让新文件建在一份已有幻灯片的母版和版式上（背景、配色、字体、标志都跟它一样，它自己的幻灯片不带过来）。已有文件不覆盖；`--output` 省略时写在「文档」文件夹，文件名是标题 |
| `read_slides` | 读一个 PowerPoint 文件（`--path lab.pptx`）作为做幻灯片的参考，不改文件：每页的标题、要点（下一级以两个空格开头）、备注和图片数，用到的版式，主题的字体和颜色，以及用它作 `--reference` 时新幻灯片会用的三个版式（`asReference`）。页多时用 `--from`、`--count` 分段读 |
| `list_manuscripts` | 列出写作页的稿件：题目、语言、各节的名称和字数 |
| `read_manuscript` | 读取一份稿件的正文，可以只读一节；只读 |
| `edit_section` | 写或修改稿件的一节：`--edits` 是一个 JSON 数组，每项是 `old`（从这一节逐字摘出的原文，留空表示加在末尾）和 `new`（改成的文字，留空表示删去）；或用 `--text` 给出这一节的全部新正文。引用写作 `[@Key]` 或 `[@文献ID]`。改动前的文字留作这一节的一个历史版本（会改动稿件） |
| `set_outline` | 设定一份稿件的大纲：`--sections` 是一个 JSON 数组，按顺序列出各节（保留的节带 `id`，可以改名、改类型、换位置；不带的是新加的），可同时给 `--title`、`--language`。没列出的节，没写过内容的去掉，写过的保留在最后（会改动稿件） |
| `find_pdf` | 为没有 PDF 的文献查找开放获取的全文（arXiv、OpenAlex 和 Europe PMC 列出的开放获取文件、部分出版商的开放获取文章），下载后附到原条目并读出文字；取回的文件是论文的补充材料（开头几页只有附图、附表）时不保存，接着试下一个地址；`--paper-id` 只处理一篇，`--limit` 一次最多几篇，`--look-only` 只列出候选地址不下载。不绕过付费墙或登录；会把 DOI 或 PMID 发给这些服务（会改动文献库） |
| `compress_pdf` | 压缩文献库里过大的 PDF：长边超过 2400 像素的图片缩到 2400 像素，照片存为 JPEG，线条图保持无损，文字、页面和标注不变，并核对每页文字与原来一致；至少小一成才替换，附件 ID 不变。`--resource-id` 只压缩一个附件，`--paper-id` 压缩一篇文献的所有 PDF。结果里 `outcome` 是 `compressed`、`not_worth` 或 `failed`，`original` 是移到 `removed/` 的原文件。带密码或权限保护的 PDF 不处理（会改动文献库） |
| `ocr_pdf` | 文字识别（OCR）：为读不出文字的 PDF（扫描件、字体没有文字编码的文件，`list_papers --fulltext textless`）读出页面上的文字，按页保存，之后 `read_fulltext`、`search_fulltext`、`find_sentences` 都用得上。引擎按桌面「设置 › 网络 › 文字识别」（`--engine` 指定一个）：DeepSeek OCR 把整个文件上传到 DeepSeek，每个文件最多 1000 页、100 MB，按页计费，Key 取「设置」里的或环境变量 `DEEPSEEK_API_KEY`；看图模型（`model`）只能在桌面应用里用。`--resource-id` 只读一个附件，`--paper-id` 读一篇文献的 PDF，都不给时处理文献库里所有读不出文字的 PDF，一次最多 `--limit` 个（默认 10，最多 50），`remaining` 是还剩几个。每个文件的 `outcome` 是 `read`（`pages`、`pagesWithText`，`cutPages` 是内容太多没读完的页）、`no_text`、`failed`（`error` 是原因），按名指定时还有 `has_text` 和 `unreadable`（设了密码等）；连续 3 个失败就停下。已有文字的文件不动；例外是按名指定的文件识别过、却有页面没读出文字时，只把这些页再送去读一次（只上传、只计这几页），读出的放回原处，结果多 `retriedPages`（送去几页）和 `filledPages`（读出几页）。 |
| `import_url` | 按网址添加一篇文献（`--url https://…`）：文献在出版商网站上的页面，或 PDF 本身的地址。网址只用本机的网络请求一次（校园网能下载的就能取到）；网页上的 `citation_doi`、`citation_pmid` 等标签给出编号，按编号查询信息后添加（已有的不重复），再下载网页标出的 PDF（`citation_pdf_url`），拿不到时找开放获取的版本（同 `find_pdf`）。网页打不开但网址里写着 DOI 时按这个 DOI 添加；PDF 地址按拖入的 PDF 处理。结果里 `outcome` 是 `added`、`attached`、`held`、`in_trash`、`not_recognised` 或 `failed`，`pdf` 是 `saved`、`had` 或 `none`（`pdfReason` 说明原因）。一个网址最多花两分钟（查询信息和找开放获取的版本都算在内），到时已经添加的文献保留，原因里会说「花的时间太长」；缺的 PDF 可以再用 `find_pdf` 找。不登录、不带 cookie、不冒充浏览器，除 PDF 地址外不跟随网页上的链接（会改动文献库） |

全文检索第一次用到某个 PDF 时会提取它的文字并缓存到库里，同时按标题把它分成导言、方法、结果、讨论并切成句子，之后即时返回。

代理用的是应用「设置 › 网络」里的同一个设置。

## MCP

把 `wisp mcp` 配置给 AI 客户端，它就能通过 MCP（标准输入输出）使用同一组工具；`wisp mcp --read-only` 不提供会改动文献库的工具。提建议的两个工具 `suggest_screening` 和 `suggest_field` 也在其中：建议虽然不改变收录判断和词条，却要写进文献库，只读时同样不提供。配置如下：

```json
{
  "mcpServers": {
    "wisp-study": { "command": "/path/to/wisp", "args": ["mcp"] }
  }
}
```

## 定时同步

不开窗口时，可以让系统的计划任务（cron、launchd、Windows 任务计划程序）定期执行 `wisp sync_searches`，它只运行已经到期的检索式，并且只处理「自动更新」开着的文献库；开关关着时这条命令什么都不做，要用 `--force`。
