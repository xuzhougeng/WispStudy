//! Wisp Study's library engine. No GPUI in here: everything can be compiled
//! and tested on its own (`cargo test -p wisp_core`).
//!
//! - `model`    papers, custom field definitions, attachments, normalisation
//! - `libraries` the independent libraries under the data folder
//! - `store`    one library on disk: a SQLite file plus `papers/`
//! - `filter`   search, filters, sorting and the sidebar counts
//! - `import`   BibTeX and RIS readers
//! - `zotero`   read-only local Zotero import, with notes, tags and PDF copies
//! - `export`   BibTeX and CSV writers
//! - `archive`  a whole library in one `.wispstudy` file (a gzipped tar) for another Wisp Study, and taking one in
//! - `extract`  提取: a model's proposals for the custom fields of a paper, each with a sentence of the source
//! - `figure`   图片库: pieces of PDF pages kept as pictures, and the caption a page gives each
//! - `excerpt`  方法库 and 导言库: methods and introductions kept in a paper's own words, leading back to their page
//! - `journals` 期刊分区与影响因子: the JCR, 中科院 and 新锐 tables the app carries, and finding a paper's journal in them
//! - `lookup`   Crossref / Europe PMC records → `Paper`
//! - `fulltext` page text of a stored PDF
//! - `pdftext`  that text read off a PDF's glyphs, with the breaks the page shows
//! - `handles`  a rectangle chosen on a PDF page (a box, a mark, a frame): its eight handles, sizing and moving it
//! - `structure` that text sorted into the parts of a paper and cut into sentences, for 写作
//! - `cite`     citations in a manuscript: keys, finding them, the reference list and BibTeX
//! - `headings` headings guessed by their look in a Word file without heading styles, for the person to confirm
//! - `pptx`     slides written as a PowerPoint file: what is on a slide is said, where it stands is decided here
//! - `docx`     Word files for 写作: read into paragraphs and comments, written again with the text as it stands and its comments
//! - `comment`  批注: remarks on words of a manuscript, the person's, the assistant's and those a Word file came with
//! - `revise`   an assistant's changes to a section of a manuscript: where each goes, and the words to light up
//! - `markdown` the Markdown of an answer → blocks to draw
//! - `panes`    分栏: the reading area as columns on an eight-unit grid, and what a window of a given width shows of a layout
//! - `table`    副表: a table kept as a file (`.xlsx`, `.csv`, `.tsv`) read row by row, for the tool `read_table`
//! - `ai`       context building, request bodies and stream parsing for 文献库提问
//! - `podcast`  播客: a paper talked over by two hosts, the script asked of a model and its sound kept as WAV
//! - `playback` the player of 朗读 and 播客: speeds, seeking, where a kept podcast goes on, speech played faster at the same pitch
//! - `speech`   朗读: a paper's text in pieces for DeepSeek's speech service, and the sound it sends
//! - `screening` the person's side of 筛选: the keys, a round of papers one by one, suggestions confirmed together
//! - `sources`  literature database searches: URLs to ask, records read from the replies
//! - `openaccess` where a paper's full text can be had for free: addresses to ask, candidates read from the replies
//! - `settings` settings.json, and where it and the libraries live
//! - `storage`  存储空间: what each library takes on the disk, and what in it takes the space
//! - `tools`    what an agent can do with a library: the one table behind the command line, MCP and the assistant
//! - `mcp`      the Model Context Protocol replies for those tools
//! - `mention`  @ in the assistant's question box: tokens naming a paper or a library, what the popup offers, what the model is told
//! - `offer`    a card of buttons the assistant puts under its answer, run only when the person presses one
//! - `connector` the person's own MCP servers the assistant may call; `oauth` signing in to one in the browser
//! - `update`   which release is the newest, and whether it is newer than this build

pub mod adjust;
pub mod ai;
pub mod annotation;
pub mod annotpdf;
pub mod archive;
pub mod cite;
pub mod collected;
pub mod citefind;
pub mod comment;
pub mod compact;
pub mod complete;
pub mod compose;
pub mod connector;
pub mod csl;
pub mod docx;
pub mod export;
pub mod excerpt;
pub mod extract;
pub mod figref;
pub mod figure;
pub mod filter;
pub mod find;
pub mod floating;
pub mod fulltext;
pub mod handles;
pub mod headings;
pub mod imagegen;
pub mod import;
pub mod journals;
pub mod judge;
pub mod libraries;
pub mod lookup;
pub mod manuscript;
pub mod markdown;
pub mod mcp;
pub mod mention;
pub mod mermaid;
pub mod mindmap;
pub mod offer;
pub mod model;
pub mod notes;
pub mod ocr;
pub mod oauth;
pub mod openaccess;
pub mod panes;
pub mod organise;
pub mod passage;
pub mod pptx;
pub mod pdftext;
pub mod provider;
pub mod restyle;
pub mod retrieval;
pub mod review;
pub mod revise;
pub mod screening;
pub mod playback;
pub mod podcast;
pub mod speech;
pub mod selection;
pub mod settings;
pub mod shrink;
pub mod skill;
pub mod slideref;
pub mod sources;
pub mod storage;
pub mod table;
pub mod store;
pub mod structure;
pub mod time;
pub mod transfer;
pub mod tools;
pub mod tour;
pub mod tutorial;
pub mod translation;
pub mod update;
pub mod usage;
pub mod webpage;
pub mod websearch;
pub mod written;
pub mod zotero;
