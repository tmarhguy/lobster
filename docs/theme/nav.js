/* Collapsible sidebar for the Asciidoctor left TOC.
   Vanilla JS, no dependencies. Safe without JS: the TOC renders fully
   expanded and every anchor link works as plain HTML. */
(function () {
  'use strict';

  // localStorage key ( bump the suffix if the scheme ever changes ).
  var STORAGE_KEY = 'techman-nav-collapsed-v1';

  var toc = document.getElementById('toc');
  if (!toc) return;
  var topList = toc.querySelector('ul.sectlevel1');
  if (!topList) return;

  // Mark JS-enhanced so CSS may hide collapsed subtrees (see docs.css).
  document.documentElement.classList.add('js-nav');

  /* --- persisted state ------------------------------------------------
     Stored as an array of section hrefs (e.g. "#_architecture") whose
     subtree is collapsed. Hrefs are stable across rebuilds as long as
     chapter titles are stable. Corrupt entries are ignored. */
  function loadCollapsed() {
    try {
      var raw = localStorage.getItem(STORAGE_KEY);
      if (!raw) return [];
      var parsed = JSON.parse(raw);
      return Array.isArray(parsed) ? parsed.filter(function (h) {
        return typeof h === 'string' && h.charAt(0) === '#';
      }) : [];
    } catch (e) {
      return [];
    }
  }
  function saveCollapsed(hrefs) {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(hrefs));
    } catch (e) {
      /* Storage unavailable (private mode, disabled): navigation still
         works for this page view, persistence is simply skipped. */
    }
  }
  function collapsedHrefs() {
    var out = [];
    topList.querySelectorAll('li.collapsed').forEach(function (li) {
      var a = li.querySelector(':scope > .toc-entry > a, :scope > a');
      if (a) out.push(a.getAttribute('href'));
    });
    return out;
  }

  /* --- structure -------------------------------------------------------
     Wrap each item's link in a .toc-entry flex row and prepend a disclosure
     button ONLY where a child list exists. The title link itself is never
     hijacked: it always navigates. */
  var COLLAPSED_LABEL = 'Expand section';
  var EXPANDED_LABEL = 'Collapse section';

  function setExpanded(li, expanded, toggle) {
    li.classList.toggle('collapsed', !expanded);
    toggle.setAttribute('aria-expanded', expanded ? 'true' : 'false');
    toggle.setAttribute('aria-label', expanded ? EXPANDED_LABEL : COLLAPSED_LABEL);
    toggle.textContent = expanded ? '\u25BE' : '\u25B8'; // ▾ / ▸
  }

  var toggles = [];
  topList.querySelectorAll('li').forEach(function (li) {
    var childList = li.querySelector(':scope > ul');
    if (!childList) return; // Leaf: no disclosure control.
    li.classList.add('has-children');

    var link = li.querySelector(':scope > a');
    var toggle = document.createElement('button');
    toggle.type = 'button';
    toggle.className = 'toc-toggle';
    toggle.setAttribute('aria-expanded', 'true');

    // Wrap link + toggle so the arrow aligns without disturbing the link.
    var row = document.createElement('span');
    row.className = 'toc-entry';
    li.insertBefore(row, link);
    row.appendChild(toggle);
    row.appendChild(link);

    toggle.addEventListener('click', function () {
      var expanded = li.classList.contains('collapsed');
      setExpanded(li, expanded, toggle);
      saveCollapsed(collapsedHrefs());
    });

    toggles.push({ li: li, toggle: toggle });
    setExpanded(li, true, toggle);
  });

  // Apply persisted state after all toggles exist.
  var stored = loadCollapsed();
  if (stored.length) {
    toggles.forEach(function (t) {
      var a = t.li.querySelector(':scope > .toc-entry > a');
      if (a && stored.indexOf(a.getAttribute('href')) !== -1) {
        setExpanded(t.li, false, t.toggle);
      }
    });
  }

  /* --- controls -------------------------------------------------------- */
  var controls = document.createElement('div');
  controls.className = 'toc-controls';
  var collapseAll = document.createElement('button');
  collapseAll.type = 'button';
  collapseAll.textContent = 'Collapse all';
  var expandAll = document.createElement('button');
  expandAll.type = 'button';
  expandAll.textContent = 'Expand all';
  controls.appendChild(collapseAll);
  controls.appendChild(expandAll);
  topList.parentNode.insertBefore(controls, topList);

  // Collapse to major chapters: only top-level subtrees collapse.
  collapseAll.addEventListener('click', function () {
    toggles.forEach(function (t) {
      if (t.li.parentNode === topList) setExpanded(t.li, false, t.toggle);
    });
    saveCollapsed(collapsedHrefs());
  });
  expandAll.addEventListener('click', function () {
    toggles.forEach(function (t) { setExpanded(t.li, true, t.toggle); });
    saveCollapsed([]);
  });

  /* --- active section --------------------------------------------------
     Expand the hierarchy containing the current section and mark its link.
     Driven by hash (clicks, back/forward, deep links) plus a scroll spy. */
  function linkForHash(hash) {
    if (!hash) return null;
    return topList.querySelector('a[href="' + hash + '"]');
  }
  function expandAncestors(link) {
    var li = link ? link.closest('li') : null;
    while (li && topList.contains(li)) {
      var t = toggles.find(function (x) { return x.li === li; });
      if (t) setExpanded(li, true, t.toggle);
      li = li.parentElement.closest('li');
    }
  }
  function markActive(link) {
    topList.querySelectorAll('a.toc-active').forEach(function (a) {
      a.classList.remove('toc-active');
    });
    if (link) {
      link.classList.add('toc-active');
      expandAncestors(link);
    }
  }
  function syncToHash() {
    var link = linkForHash(window.location.hash);
    if (link) markActive(link);
  }
  window.addEventListener('hashchange', syncToHash);

  // Scroll spy: the topmost visible heading wins. Headings already carry
  // ids from :sectanchors:, so no DOM mutation is needed for tracking.
  var headings = Array.prototype.slice.call(
    document.querySelectorAll('#content h2[id], #content h3[id], #content h4[id]')
  );
  var ticking = false;
  function spy() {
    ticking = false;
    var current = null;
    // Document-relative heading tops (robust against positioned ancestors).
    var y = window.scrollY + 96; // Slight offset below the top edge.
    for (var i = 0; i < headings.length; i++) {
      var top = headings[i].getBoundingClientRect().top + window.scrollY;
      if (top <= y) current = headings[i];
      else break;
    }
    // Only auto-mark while the user scrolls; never rewrite the URL hash.
    if (current) markActive(linkForHash('#' + current.id));
  }
  window.addEventListener('scroll', function () {
    if (!ticking) {
      ticking = true;
      window.requestAnimationFrame(spy);
    }
  }, { passive: true });

  syncToHash();
})();
