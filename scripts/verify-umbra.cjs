// umbra 화면 검증 — 구조를 감지해서 해당하는 검사만 돌린다.
//
//   node scripts/verify-screen.js <파일경로 또는 URL>
//
// 사전 준비:  npm install playwright
//
// 값 감사(audit.sh)가 못 잡는 것을 잡는다. 실제로 5개 화면을 만들어 보니
// 값·로직 검사를 전부 통과하고도 이 스위트가 버그를 여러 개 찾아냈다:
//   · 클릭 가능한 테이블 행이 키보드로 도달 불가
//   · 세그먼트 스위치의 선택 상태가 스크린리더에 전달 안 됨
//   · 긴 목록에서 헤더가 사라짐
//   · 라벨 대비가 하한선 미달
//
// 이 스위트도 전부 통과했는데 스크린샷에서만 보인 버그가 또 있었다.
// **마지막에는 반드시 눈으로 봐라.**

const { chromium } = require("playwright");
const path = require("path");

const ARG = process.argv[2];
if (!ARG) { console.error("사용법: node scripts/verify-screen.js <파일경로 또는 URL>"); process.exit(2); }
const URL = /^https?:|^file:/.test(ARG) ? ARG : "file:///" + path.resolve(ARG).replace(/\\/g, "/");

let fail = 0;
const ok  = (k, v) => console.log("  " + String(k).padEnd(26) + v + "  ✔");
const bad = (k, v) => { fail++; console.log("  " + String(k).padEnd(26) + v + "  ✘"); };
const info = (k, v) => console.log("  " + String(k).padEnd(26) + v);
const head = t => console.log("\n═══ " + t + " " + "═".repeat(Math.max(0, 46 - t.length)));

// 페이지 안에서 쓰는 대비 계산기
function inject() {
  window.__cr = (fg, bg) => {
    const L = c => c.match(/\d+/g).slice(0, 3)
      .map(n => { n /= 255; return n <= .03928 ? n / 12.92 : Math.pow((n + .055) / 1.055, 2.4); })
      .reduce((a, v, i) => a + [.2126, .7152, .0722][i] * v, 0);
    const x = L(fg), y = L(bg);
    return +((Math.max(x, y) + .05) / (Math.min(x, y) + .05)).toFixed(2);
  };
}

(async () => {
  const browser = await chromium.launch({executablePath:"C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe"});
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const errors = [];
  page.on("console", m => { if (m.type() === "error") errors.push(m.text()); });
  page.on("pageerror", e => errors.push("PAGEERROR " + e.message));

  await page.goto(URL);
  await page.waitForTimeout(900);
  await page.evaluate(inject);

  const has = await page.evaluate(() => ({
    shell:  !!document.querySelector(".app-shell, .gcs-shell"),
    topbar: !!document.querySelector(".topbar"),
    rail:   !!document.querySelector(".nav-rail"),
    toolbar:!!document.querySelector(".view-toolbar, .map-toolbar"),
    tiles:  !!document.querySelector(".metric-grid.wide"),
    table:  !!document.querySelector("table"),
    fields: !!document.querySelector(".field"),
    seg:    !!document.querySelector(".segmented, .map-switch"),
    panel:  !!document.querySelector(".side-panel, .telemetry")
  }));

  console.log("검사 대상: " + URL);
  console.log("감지: " + Object.entries(has).filter(([, v]) => v).map(([k]) => k).join(" · "));

  /* ── 셸 규격 ─────────────────────────────────── */
  if (has.shell) {
    head("셸 규격");
    const d = await page.evaluate(() => {
      const g = s => { const e = document.querySelector(s); return e ? Math.round(e.getBoundingClientRect().height) : null; };
      const w = s => { const e = document.querySelector(s); return e ? Math.round(e.getBoundingClientRect().width) : null; };
      return { top: g(".topbar"), tool: g(".view-toolbar, .map-toolbar"), rail: w(".nav-rail"), panel: w(".side-panel, .telemetry") };
    });
    d.top   !== null && (d.top === 60 ? ok("상단 바", "60px") : bad("상단 바", d.top + "px (60 이어야 함)"));
    d.tool  !== null && (d.tool === 52 ? ok("툴바", "52px") : bad("툴바", d.tool + "px (52 이어야 함)"));
    d.rail  !== null && (d.rail === 58 ? ok("레일", "58px") : bad("레일", d.rail + "px (58 이어야 함)"));
    d.panel !== null && (d.panel === 318 ? ok("사이드 패널", "318px") : bad("사이드 패널", d.panel + "px (318 이어야 함)"));
  }

  /* ── 툴바 계약 ───────────────────────────────── */
  if (has.toolbar) {
    head("툴바 계약");
    const t = await page.evaluate(() => {
      const eb = document.querySelector(".eyebrow");
      const ti = document.querySelector(".view-heading strong, .map-heading strong");
      const ce = document.querySelector(".toolbar-center");
      const active = document.querySelector(".nav-rail button.active");
      return {
        eyebrow: eb ? getComputedStyle(eb).fontSize + " " + getComputedStyle(eb).color : null,
        title: ti ? ti.textContent.trim() : null,
        center: ce ? ce.textContent.trim().length : null,
        rail: active ? (active.getAttribute("aria-label") || "").trim() : null
      };
    });
    t.title && info("제목", t.title);
    if (t.rail) (t.title === t.rail ? ok("제목 == 레일 라벨", t.rail) : bad("제목 != 레일 라벨", `"${t.title}" vs "${t.rail}"`));
    t.center !== null && (t.center > 0 ? ok("툴바 가운데", "채움") : bad("툴바 가운데", "비었음 — 맥락 메타라도 넣어라"));
  }

  /* ── 정보 위계 ───────────────────────────────── */
  head("정보 위계");
  const h = await page.evaluate(() => {
    const big = [...document.querySelectorAll("strong")].filter(e => {
      const c = getComputedStyle(e);
      return parseFloat(c.fontSize) >= 16 && c.fontFamily.toLowerCase().includes("mono") && e.offsetParent;
    });
    return { n: big.length, vals: big.slice(0, 6).map(e => e.textContent.trim().slice(0, 10)),
             tiles: document.querySelectorAll(".metric-grid.wide > div").length };
  });
  if (h.n === 0) ok("1초 레벨", "0개 (조작 화면)");
  else if (h.n <= 5) ok("1초 레벨", h.n + "개 — " + h.vals.join(" | "));
  else bad("1초 레벨", h.n + "개 (4~5개여야 함) — 위계를 다시 짜라");
  if (has.tiles) (h.tiles === 4 ? ok("지표 타일", "4개") : bad("지표 타일", h.tiles + "개 (레시피 A는 4개)"));

  /* ── 대비 하한 ───────────────────────────────── */
  head("대비 하한 3:1");
  const low = await page.evaluate(() => {
    const out = [];
    document.querySelectorAll("span,strong,small,td,th,em,b,label,div").forEach(e => {
      if (!e.offsetParent || !e.childNodes.length) return;
      const txt = [...e.childNodes].filter(n => n.nodeType === 3 && n.textContent.trim()).map(n => n.textContent.trim()).join("");
      if (!txt) return;
      const c = getComputedStyle(e);
      // 반투명 배경은 아래 레이어와 합성해야 한다. 안 하면 오탐이 난다.
      const layers = [];
      let p = e;
      while (p && p !== document.documentElement) {
        const b = getComputedStyle(p).backgroundColor;
        const m = b && b.match(/[\d.]+/g);
        if (m && +(m[3] ?? 1) > 0) { layers.push([+m[0], +m[1], +m[2], +(m[3] ?? 1)]); if (+(m[3] ?? 1) === 1) break; }
        p = p.parentElement;
      }
      let bgc = [9, 13, 15];                       // 최하층 = body 배경
      for (let i = layers.length - 1; i >= 0; i--) {
        const [r0, g0, b0, a] = layers[i];
        bgc = [r0 * a + bgc[0] * (1 - a), g0 * a + bgc[1] * (1 - a), b0 * a + bgc[2] * (1 - a)];
      }
      const r = window.__cr(c.color, "rgb(" + bgc.map(Math.round).join(",") + ")");
      if (r < 3) out.push(txt.slice(0, 18) + " " + c.fontSize + " " + r);
    });
    return [...new Set(out)].slice(0, 8);
  });
  low.length === 0 ? ok("3:1 미만", "없음") : bad("3:1 미만", low.length + "건 — " + low.join(" / "));

  // AA(4.5) 미만은 경고로만 — 라벨은 의도적으로 흐릴 수 있다.
  // 단 "값의 이름표"라면 4.53 으로 올려라 (tokens.md).
  const sub = await page.evaluate(() => {
    const out = [];
    document.querySelectorAll("span,small,th,td,em,b,label").forEach(e => {
      if (!e.offsetParent) return;
      const txt = [...e.childNodes].filter(n => n.nodeType === 3 && n.textContent.trim()).map(n => n.textContent.trim()).join("");
      if (!txt) return;
      const c = getComputedStyle(e);
      let p = e, bg = "rgb(17,23,25)";
      while (p && p !== document.documentElement) {
        const b = getComputedStyle(p).backgroundColor, m = b && b.match(/[\d.]+/g);
        if (m && +(m[3] ?? 1) === 1) { bg = b; break; }
        p = p.parentElement;
      }
      const r = window.__cr(c.color, bg);
      if (r >= 3 && r < 4.5) out.push(txt.slice(0, 16) + " " + c.fontSize + " " + r);
    });
    return [...new Set(out)].slice(0, 6);
  });
  if (sub.length === 0) {
    ok("AA(4.5) 미만", "없음");
  } else {
    info("AA(4.5) 미만", sub.length + "건 (경고) — " + sub.join(" / "));
    info("", "→ 값의 이름표라면 #768187(4.53)로 올려라 (tokens.md)");
  }

  /* ── 키보드 ─────────────────────────────────── */
  head("키보드 접근");
  await page.evaluate(() => { const a = document.activeElement; if (a && a.blur) a.blur(); });
  let n = 0, ring = 0; const miss = [];
  for (let i = 0; i < 40; i++) {
    await page.keyboard.press("Tab");
    const s = await page.evaluate(() => {
      const e = document.activeElement; if (!e || e === document.body) return null;
      const c = getComputedStyle(e);
      return { l: (e.getAttribute("aria-label") || e.textContent || e.value || e.tagName).trim().slice(0, 18),
               // Native video controls retain focus inside the browser's closed shadow root.
               ok: (e.matches(":focus-visible") || (e.tagName === "VIDEO" && e.matches(":focus-within"))) && c.outlineWidth !== "0px" && c.outlineStyle !== "none" };
    });
    if (!s) break;
    n++; s.ok ? ring++ : miss.push(s.l);
  }
  info("Tab 도달", n + "개");
  n === 0 ? ok("포커스 링", "인터랙티브 요소 없음 · 해당 없음") : ring === n ? ok("포커스 링", ring + "/" + n) : bad("포커스 링", ring + "/" + n + " — 누락: " + miss.join(", "));

  // 로빙 탭인덱스(한 행만 0, 나머지는 명시적 -1)는 가상 스크롤 목록의 정상 패턴이다.
  // aria-hidden 스페이서 행은 클릭 대상이 아니다.
  const rows = await page.evaluate(() => {
    const trs = [...document.querySelectorAll("tr[onclick], tbody tr")]
      .filter(t => t.getAttribute("aria-hidden") !== "true" && getComputedStyle(t).cursor === "pointer");
    const roving = trs.some(t => t.tabIndex === 0) && trs.every(t => t.hasAttribute("tabindex"));
    return { total: trs.length, roving, missing: trs.filter(t => !t.hasAttribute("tabindex")).length };
  });
  rows.missing === 0
    ? ok("클릭 가능한 행", rows.roving ? "로빙 탭인덱스 (" + rows.total + "행 · 방향키 이동)" : "전부 키보드 도달 가능")
    : bad("클릭 가능한 행", rows.missing + "개가 tabindex 없음 — 마우스 전용");

  /* ── 세그먼트 스위치 ─────────────────────────── */
  if (has.seg) {
    head("세그먼트 스위치 접근성");
    const s = await page.evaluate(() => [...document.querySelectorAll(".segmented, .map-switch")].map(g => ({
      role: g.getAttribute("role"), label: g.getAttribute("aria-label"),
      pressed: [...g.querySelectorAll("button")].every(b => b.hasAttribute("aria-pressed"))
    })));
    s.forEach((g, i) => {
      const p = "그룹 " + (i + 1);
      g.role === "group" && g.label ? ok(p + " role/label", g.label) : bad(p + " role/label", (g.role || "없음") + " / " + (g.label || "없음"));
      g.pressed ? ok(p + " aria-pressed", "전부 있음") : bad(p + " aria-pressed", "누락 — 선택 상태 전달 안 됨");
    });
  }

  /* ── 테이블 ─────────────────────────────────── */
  if (has.table) {
    head("테이블");
    // sticky 헤더는 긴 목록의 요구다 — 12행 이하의 짧은 표(비교표 등)에는 묻지 않는다
    const nrows = await page.evaluate(() => Math.max(0, ...[...document.querySelectorAll("table")].map(t => t.tBodies[0] ? t.tBodies[0].rows.length : 0)));
    const t = await page.evaluate(() => {
      const th = document.querySelector("thead th");
      const sc = document.querySelector(".scroller") || document.scrollingElement;
      const before = th ? th.getBoundingClientRect().top : null;
      if (sc && sc.scrollTop !== undefined) sc.scrollTop = 400;
      // 스크롤이 아예 안 되면 헤더가 "안 움직여서" sticky 가 통과한다 — 실제로 속았다.
      // 내용이 넘치는데 scrollTop 이 0 이면 스크롤러가 내용만큼 자란 것이다 (그리드 아이템 min-height: 0 누락).
      // 스크롤러가 내용만큼 자라면 자기 자신은 넘치는 게 없다 — 넘침이 조상으로 옮겨가 body overflow:hidden 에 잘린다.
      // 그래서 scrollHeight 비교로는 못 잡는다. "스크롤러가 뷰포트보다 큰가" 로 본다. (되살린 버그로 검증)
      const cannotScroll = !!sc && sc !== document.scrollingElement && sc.getBoundingClientRect().height > window.innerHeight + 1;
      return { cannotScroll, pos: th ? getComputedStyle(th).position : null,
               moved: th ? Math.round(Math.abs(th.getBoundingClientRect().top - before)) : null,
               wrap: (() => { let n = document.querySelector("table"); while (n && n !== document.body) {
                        n = n.parentElement; if (n && /auto|scroll/.test(getComputedStyle(n).overflowX)) return true; } return false; })() };
    });
    if (nrows <= 12) { ok("헤더 sticky", "짧은 표 (" + nrows + "행) · 해당 없음"); }
    else {
    if (t.cannotScroll) bad("스크롤러", "뷰포트보다 크다 — 내용만큼 자라서 스크롤이 안 된다. 그리드 아이템에 min-height: 0, 행 트랙에 minmax(0, 1fr)");
    t.pos === "sticky" && t.moved !== null && t.moved <= 1 && !t.cannotScroll
      ? ok("헤더 sticky", "적용됨 · 스크롤 후 " + t.moved + "px 이동")
      : bad("헤더 sticky", (t.pos || "없음") + (t.moved ? " · 스크롤 시 " + t.moved + "px 밀림" : "") + " — 긴 목록에서 열 이름이 사라진다");
    t.wrap ? ok("가로 스크롤 래퍼", "있음") : bad("가로 스크롤 래퍼", "없음 — overflow-x: auto 인 조상으로 감싸라 (.table-wrap 또는 스크롤러 자체)");
    }
    await page.evaluate(() => { const sc = document.querySelector(".scroller"); if (sc) sc.scrollTop = 0; });
  }

  /* ── 폼 ─────────────────────────────────────── */
  if (has.fields) {
    head("폼");
    const f = await page.evaluate(() => {
      const fs = [...document.querySelectorAll(".field")];
      return { total: fs.length,
        unlabeled: fs.filter(x => !x.closest("label") && !x.getAttribute("aria-label") && !x.labels?.length).length,
        numNoMode: fs.filter(x => /^[\d.,]+$/.test(x.value) && !x.getAttribute("inputmode")).length };
    });
    f.unlabeled === 0 ? ok("라벨 연결", f.total + "개 전부") : bad("라벨 연결", f.unlabeled + "개 미연결");
    f.numNoMode === 0 ? ok("숫자 inputmode", "적용됨") : bad("숫자 inputmode", f.numNoMode + "개 누락 — 모바일에서 문자 키보드");
  }

  /* ── 반응형 ─────────────────────────────────── */
  head("반응형");
  // 문서 가로 넘침만 보면 스크롤 컨테이너 안의 넘침을 놓친다 — 표가 스크롤러보다 100픽셀 넓어 마지막 열이 잘렸는데
  // 이 검사가 통과했다 (스크린샷에서만 보였다). 세 가지를 더 본다:
  //   · 표가 자기 스크롤 컨테이너보다 넓은가 (.table-wrap 은 가로 스크롤이 설계이므로 제외)
  //   · overflow: hidden 안에서 텍스트가 잘리는가 (ellipsis 는 정상, opacity 0 계열은 접힌 UI 라 제외)
  //   · 뷰포트 오른쪽 밖으로 나간 요소 (transform 으로 일부러 밀어낸 드로어는 제외)
  for (const [w, hh, name] of [[1440, 900, "기본"], [1100, 900, "≤1180"], [900, 900, "≤930"], [1440, 740, "≤780h"]]) {
    await page.setViewportSize({ width: w, height: hh });
    await page.waitForTimeout(300);
    const r = await page.evaluate(() => {
      const out = { docOver: document.documentElement.scrollWidth > document.documentElement.clientWidth, clip: [], vpOut: [], tableOver: [] };
      const vw = document.documentElement.clientWidth;
      const up = (el, f) => { for (let n = el; n && n !== document.body; n = n.parentElement) if (f(getComputedStyle(n))) return true; return false; };
      const invisible = el => up(el, c => c.opacity === "0" || c.visibility === "hidden" || c.display === "none");
      const transformed = el => up(el, c => c.transform && c.transform !== "none");
      const nm = el => el.tagName.toLowerCase() + (typeof el.className === "string" && el.className ? "." + el.className.trim().split(/\s+/).slice(0, 2).join(".") : "");
      for (const el of document.querySelectorAll("body *")) {
        const c = getComputedStyle(el); if (c.display === "none" || c.visibility === "hidden") continue;
        const b = el.getBoundingClientRect(); if (!b.width || !b.height || invisible(el)) continue;
        const hasText = [...el.childNodes].some(n => n.nodeType === 3 && n.textContent.trim());
        if (/hidden|clip/.test(c.overflowX) && hasText && el.scrollWidth > el.clientWidth + 1 && c.textOverflow !== "ellipsis") out.clip.push(nm(el) + " " + el.clientWidth + "<" + el.scrollWidth);
        if (b.right > vw + 1 && !transformed(el)) out.vpOut.push(nm(el) + " right=" + Math.round(b.right));
      }
      // 그리드·플렉스 자식이 컨테이너 오른쪽 밖으로 나감 — 열을 지정하지 않은 그리드의 암묵 열이 nowrap 내용만큼
      // 자라면 스크롤러·표가 통째로 따라 넓어진다 (툴바 판독값 때문에 STATE 열이 잘렸다). 절대·고정 배치 자식은 제외.
      out.gridOver = [];
      for (const el of document.querySelectorAll("body *")) {
        const c = getComputedStyle(el); if (!/grid|flex/.test(c.display) || c.display === "none") continue;
        const pr = el.getBoundingClientRect(); if (!pr.width) continue;
        for (const ch of el.children) {
          const cc = getComputedStyle(ch); if (/absolute|fixed/.test(cc.position) || cc.display === "none" || invisible(ch) || transformed(ch)) continue;
          const cr = ch.getBoundingClientRect(); if (!cr.width) continue;
          if (cr.right > pr.right + 1) { out.gridOver.push(nm(ch) + " " + Math.round(cr.right) + ">" + Math.round(pr.right) + " (in " + nm(el) + ")"); break; }
        }
      }
      for (const t of document.querySelectorAll("table")) {
        if (!t.offsetWidth) continue;
        let n = t.parentElement; while (n && n !== document.body && !/auto|scroll|hidden/.test(getComputedStyle(n).overflowX)) n = n.parentElement;
        if (n && n !== document.body && !n.classList.contains("table-wrap") && t.offsetWidth > n.clientWidth + 1) out.tableOver.push(nm(t) + " " + t.offsetWidth + ">" + n.clientWidth);
      }
      return out;
    });
    const probs = [];
    if (r.docOver) probs.push("문서 가로 넘침");
    if (r.gridOver.length) probs.push("자식이 컨테이너 밖으로 " + r.gridOver.length + "건: " + r.gridOver.slice(0, 2).join(", ") + " — 그리드 열에 minmax(0, 1fr), 내용은 줄여라");
    if (r.tableOver.length) probs.push("표가 스크롤러보다 넓음 " + r.tableOver.slice(0, 2).join(", ") + " — table-layout: fixed");
    if (r.clip.length) probs.push("텍스트 잘림 " + r.clip.length + "건: " + r.clip.slice(0, 2).join(", "));
    if (r.vpOut.length) probs.push("뷰포트 밖 " + r.vpOut.length + "건: " + r.vpOut.slice(0, 2).join(", "));
    probs.length ? bad(name + " (" + w + "×" + hh + ")", probs.join(" · ")) : ok(name + " (" + w + "×" + hh + ")", "넘침·잘림 없음");
  }
  await page.setViewportSize({ width: 1440, height: 900 });

  /* ── 콘솔 ───────────────────────────────────── */
  head("콘솔");
  errors.length === 0 ? ok("오류", "0건") : bad("오류", errors.length + "건 — " + errors[0].slice(0, 70));

  /* ── 스크린샷 ───────────────────────────────── */
    // 스크린샷은 실행 폴더가 아니라 전용 폴더에 — 저장소 루트에서 돌리면 png 가 커밋에 딸려 들어간다 (실제로 38장 들어갔다)
  const shotDir = path.join(process.cwd(), ".verify-shots"); require("fs").mkdirSync(shotDir, { recursive: true });
  const shot = path.join(shotDir, "verify-" + Date.now() + ".png");
  await page.mouse.move(700, 780);
  await page.waitForTimeout(200);
  await page.screenshot({ path: shot, fullPage: false, mask: [page.locator("video"), page.locator('input[type="password"]')] });

  console.log("\n" + "─".repeat(56));
  console.log(fail === 0 ? "  전 항목 통과" : "  실패 " + fail + "건");
  console.log("  스크린샷: " + shot);
  console.log("  ⚠ 이 스위트를 통과해도 눈으로만 보이는 버그가 있다. 스크린샷을 반드시 확인해라.");
  console.log("─".repeat(56));

  await browser.close();
  process.exit(fail === 0 ? 0 : 1);
})().catch(e => { console.error("ERR", e.message); process.exit(2); });

