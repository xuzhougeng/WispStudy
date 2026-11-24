// The page works without this file: every step is then shown one under the
// other and the download buttons lead to the release page.
(() => {
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
  document.documentElement.classList.add('js');

  // The words this script writes, in the page's language: site/en/ holds the
  // English pages, which load this same file from one folder up.
  const SAY = document.documentElement.lang === 'en' ? {
    chip: label => 'Screening: ' + label,
    taken: label => `Recorded: ${label}. You took the assistant's suggestion. The reason stays the paragraph it wrote, and you can change it at any time in the paper's details.`,
    other: label => `Recorded: ${label}. The assistant suggested “Related”. You chose another, and yours is the one that counts.`,
    hero: tag => 'Download ' + tag,
    day: date => date.toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' }),
    latest: (tag, day) => `Latest version ${tag}${day ? ', released ' + day : ''}. `,
    notes: 'What changed in this version',
    stop: '.',
    button: (label, kind) => label ? (kind ? `${label} (.${kind})` : label) : (kind ? 'Download .' + kind : 'Download'),
    gap: '\u2003',
  } : {
    chip: label => '收录：' + label,
    taken: label => '记下了：' + label + '。你采纳了助手的建议，判断理由沿用它写的那一段，之后在文献详情里随时可以改。',
    other: label => '记下了：' + label + '。助手建议的是「相关」，你另选了一个，以你的为准。',
    hero: tag => '下载 ' + tag,
    day: date => date.toLocaleDateString('zh-CN', { year: 'numeric', month: 'long', day: 'numeric' }),
    latest: (tag, day) => `最新版本 ${tag}${day ? '，' + day + '发布' : ''}。`,
    notes: '这一版改了什么',
    stop: '。',
    button: (label, kind) => (label ? label + ' ' : '') + (kind ? '下载 .' + kind : '下载'),
    gap: '\u3000',
  };
  // release.json lies beside this file, whichever page loaded it.
  const RELEASE = new URL('release.json', document.currentScript ? document.currentScript.src : location.href);

  // Run the highlighter over an element once more.
  function light(el) {
    if (!el) return;
    el.classList.remove('lit');
    void el.offsetWidth;
    el.classList.add('lit');
  }

  // ---- the eight steps: a tab list ----
  const tabs = $$('.steps [role=tab]');
  const panels = tabs.map(tab => document.getElementById(tab.getAttribute('aria-controls')));
  function show(index, focus) {
    tabs.forEach((tab, i) => {
      const on = i === index;
      tab.setAttribute('aria-selected', on);
      tab.tabIndex = on ? 0 : -1;
      panels[i].hidden = !on;
    });
    if (focus) tabs[index].focus();
    const list = tabs[index].parentElement;
    if (list.scrollWidth > list.clientWidth) list.scrollTo({ left: tabs[index].offsetLeft - list.clientWidth / 2 + tabs[index].offsetWidth / 2, behavior: 'smooth' });
  }
  tabs.forEach((tab, i) => {
    tab.addEventListener('click', () => show(i));
    tab.addEventListener('keydown', event => {
      const step = { ArrowDown: 1, ArrowRight: 1, ArrowUp: -1, ArrowLeft: -1 }[event.key];
      const to = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : step ? (i + step + tabs.length) % tabs.length : null;
      if (to === null) return;
      event.preventDefault();
      show(to, true);
    });
  });
  if (tabs.length) {
    show(0);
    // The other screenshots, once the page has settled, so a step opens at once.
    addEventListener('load', () => setTimeout(() => panels.forEach(panel => $$('img', panel).forEach(img => { new Image().src = img.src; })), 1200));
  }

  // ---- citations of this page's own figures ----
  $$('a.cite').forEach(link => link.addEventListener('click', event => {
    const target = document.getElementById(link.hash.slice(1));
    if (!target) return;
    event.preventDefault();
    const step = link.dataset.step;
    if (step) show(tabs.findIndex(tab => tab.id === 'tab-' + step));
    (step ? $('#fig3') : target).scrollIntoView({ behavior: 'smooth', block: step ? 'start' : 'center' });
    setTimeout(() => light($('figcaption', target)), 450);
  }));

  // ---- 图 4: a suggestion, and the person's decision ----
  const verdict = $('#screen-verdict');
  const chip = $('#screen-chip');
  $$('.choices button').forEach(button => button.addEventListener('click', () => {
    $$('.choices button').forEach(other => other.setAttribute('aria-pressed', other === button));
    const label = button.dataset.label;
    chip.className = 'chip ' + button.dataset.tone;
    chip.textContent = SAY.chip(label);
    verdict.textContent = button.classList.contains('suggested') ? SAY.taken(label) : SAY.other(label);
    light(verdict);
  }));

  // ---- 图 5: a citation that opens the page and marks the sentence ----
  $$('button.cite[data-quote]').forEach(button => button.addEventListener('click', () => {
    $$('button.cite[data-quote]').forEach(other => other.setAttribute('aria-pressed', other === button));
    $$('.sheet mark').forEach(mark => mark.classList.toggle('on', mark.id === button.dataset.quote));
    const mark = document.getElementById(button.dataset.quote);
    const box = mark.getBoundingClientRect();
    if (box.top < 80 || box.bottom > innerHeight - 40) mark.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }));

  // ---- downloads: this visitor's system, and the latest release ----
  const platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || '';
  const agent = navigator.userAgent || '';
  const mobile = /Android|iPhone|iPad|iPod/.test(agent) || (/Mac/.test(platform) && navigator.maxTouchPoints > 1);
  const os = mobile ? '' : /Mac/i.test(platform) ? 'mac' : /Win/i.test(platform) ? 'win' : /Linux/i.test(platform) ? 'linux' : '';
  if (os) $(`.dl[data-os=${os}]`).classList.add('mine');

  // release.json is written when the site is deployed (see .github/workflows/pages.yml).
  // Earlier releases had one universal DMG; both Mac buttons lead to it there.
  const WANTED = { mac: [/macos.*arm64/, /macos-universal/], 'mac-intel': [/macos.*x64/, /macos-universal/], win: [/windows.*\.zip$/], 'win-setup': [/windows.*setup\.exe$/], linux: [/linux/] };
  fetch(RELEASE).then(response => response.ok ? response.json() : null).then(release => {
    if (!release || !release.tag || !Array.isArray(release.assets)) return;
    const version = $('[data-version]');
    version.textContent = release.tag;
    version.hidden = false;
    $('#hero-download').textContent = SAY.hero(release.tag);
    const day = release.published ? SAY.day(new Date(release.published)) : '';
    const line = $('#release-line');
    line.textContent = SAY.latest(release.tag, day);
    const notes = document.createElement('a');
    notes.href = release.url;
    notes.textContent = SAY.notes;
    line.append(notes, SAY.stop);
    for (const [key, patterns] of Object.entries(WANTED)) {
      const asset = patterns.map(pattern => release.assets.find(a => pattern.test(a.name))).find(Boolean);
      if (!asset) {
        // A release from before there was a setup: nothing is offered that it does not have.
        if (key === 'win-setup') $$(`[data-asset="${key}"], [data-file="${key}"], [data-with="${key}"]`).forEach(el => el.remove());
        continue;
      }
      const kind = (asset.name.match(/\.(dmg|zip|tar\.gz|exe)$/) || [])[1];
      const button = $(`[data-asset="${key}"]`);
      button.href = asset.url;
      button.textContent = SAY.button(button.dataset.label || '', kind);
      $(`[data-file="${key}"]`).textContent = `${asset.name}${SAY.gap}${(asset.size / 1048576).toFixed(1)} MB`;
    }
  }).catch(() => {});
})();
