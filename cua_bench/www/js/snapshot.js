// cua-bench snapshot and augmented actions builder
// Defines window.__td_build_snapshot which returns a serialized, normalized HTML string

(function(){
  if (window.__td_build_snapshot) return; // idempotent

  // Function to check if element has text cursor and get valid drag points
  function getTextDragPoints(el) {
    const computedStyle = window.getComputedStyle(el);
    const cursor = computedStyle.cursor;

    // Check if cursor is a text cursor
    const textCursors = ['text', 'auto']; // 'auto' can be text depending on element
    if (!textCursors.includes(cursor)) {
      return null;
    }

    // Get all text nodes
    const textContent = el.textContent || '';
    if (textContent.length < 2) {
      return null; // Need at least 2 characters
    }

    // Try up to 10 times to find a valid range
    for (let attempt = 0; attempt < 10; attempt++) {
      // Select 2 random character positions
      const pos1 = Math.floor(Math.random() * textContent.length);
      const pos2 = Math.floor(Math.random() * textContent.length);

      if (pos1 === pos2) continue;

      const startPos = Math.min(pos1, pos2);
      const endPos = Math.max(pos1, pos2);

      try {
        const range = document.createRange();

        // Find the text node and offsets
        let currentOffset = 0;
        let startNode = null, startOffset = 0;
        let endNode = null, endOffset = 0;

        const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
        let node;

        while (node = walker.nextNode()) {
          const nodeLength = node.textContent.length;

          if (!startNode && currentOffset + nodeLength > startPos) {
            startNode = node;
            startOffset = startPos - currentOffset;
          }

          if (!endNode && currentOffset + nodeLength >= endPos) {
            endNode = node;
            endOffset = endPos - currentOffset;
            break;
          }

          currentOffset += nodeLength;
        }

        if (!startNode || !endNode) continue;

        // Create ranges for first and last character
        const firstCharRange = document.createRange();
        firstCharRange.setStart(startNode, startOffset);
        firstCharRange.setEnd(startNode, startOffset + 1);

        const lastCharRange = document.createRange();
        lastCharRange.setStart(endNode, endOffset - 1);
        lastCharRange.setEnd(endNode, endOffset);

        const firstBox = firstCharRange.getBoundingClientRect();
        const lastBox = lastCharRange.getBoundingClientRect();

        if (firstBox.width === 0 || lastBox.width === 0) continue;

        // Calculate points: x=0% y=50% for first, x=50% y=50% for last
        const point1 = {
          x: firstBox.left,
          y: firstBox.top + firstBox.height * 0.5
        };

        const point2 = {
          x: lastBox.left + lastBox.width * 0.5,
          y: lastBox.top + lastBox.height * 0.5
        };

        // Check if both points hit the same element
        const hitEl1 = document.elementFromPoint(point1.x, point1.y);
        const hitEl2 = document.elementFromPoint(point2.x, point2.y);

        if (hitEl1 === el && hitEl2 === el) {
          // Get the full range text
          range.setStart(startNode, startOffset);
          range.setEnd(endNode, endOffset);
          const rangeText = range.toString();

          // Clear selection
          window.getSelection().removeAllRanges();

          return {
            point1: point1,
            point2: point2,
            text: rangeText,
            success: true
          };
        }
      } catch (e) {
        continue;
      }
    }

    return null;
  }

  function buildSnapshot(options) {
    const {
      includeClientRects = true,
      includeKeyboardFocus = true,
      includeFormValues = true,
      includeScrollOffset = true,
      includeTextSelection = true,
      includeRandomTextActions = true,
      includeWindowManagementActions = true,
      forceWindowPosition = false,
      screenOffsetX = 0,
      screenOffsetY = 0,
    } = options || {};
    // Clone the document
    const clone = document.documentElement.cloneNode(true);

    // Add window scroll position to the root element
    if (includeScrollOffset) {
      clone.setAttribute('data-window-scroll-x', window.scrollX);
      clone.setAttribute('data-window-scroll-y', window.scrollY);
    }

    // Add window screen position to the root element
    if (includeClientRects || forceWindowPosition) {
      if (!forceWindowPosition) {
        clone.setAttribute('data-window-screen-x', (window.screenX || 0) + screenOffsetX);
        clone.setAttribute('data-window-screen-y', (window.screenY || 0) + screenOffsetY);
      } else {
        clone.setAttribute('data-window-screen-x', screenOffsetX);
        clone.setAttribute('data-window-screen-y', screenOffsetY);
      }
    }

    // Helper to find corresponding element in clone
    function getCloneElement(original, cloneRoot) {
      const path = [];
      let node = original;
      // Build path from original to root
      while (node && node !== document.documentElement) {
        const parent = node.parentElement;
        if (parent) {
          const index = Array.from(parent.children).indexOf(node);
          path.unshift(index);
        }
        node = parent;
      }
      // Navigate clone using path
      let cloneNode = cloneRoot;
      for (const index of path) {
        cloneNode = cloneNode.children[index];
      }
      return cloneNode;
    }

    // Normalize all input elements
    if (includeFormValues) {
      document.querySelectorAll('input, textarea').forEach(el => {
        const cloneEl = getCloneElement(el, clone);
        if (!cloneEl) return;
        if (el.type === 'checkbox' || el.type === 'radio') {
          if (el.checked) {
            cloneEl.setAttribute('checked', '');
          } else {
            cloneEl.removeAttribute('checked');
          }
        } else {
          cloneEl.setAttribute('value', el.value);
        }
      });
    }

    // Normalize select elements
    if (includeFormValues) {
      document.querySelectorAll('select').forEach(el => {
        const cloneEl = getCloneElement(el, clone);
        if (!cloneEl) return;
        const options = el.querySelectorAll('option');
        const cloneOptions = cloneEl.querySelectorAll('option');
        options.forEach((opt, i) => {
          if (opt.selected) {
            cloneOptions[i].setAttribute('selected', '');
          } else {
            cloneOptions[i].removeAttribute('selected');
          }
        });
      });
    }

    // Add scroll positions
    if (includeScrollOffset) {
      document.querySelectorAll('*').forEach(el => {
        if (el.scrollTop > 0 || el.scrollLeft > 0) {
          const cloneEl = getCloneElement(el, clone);
          if (!cloneEl) return;
          cloneEl.setAttribute('data-scroll-top', el.scrollTop);
          cloneEl.setAttribute('data-scroll-left', el.scrollLeft);
        }
      });
    }

    // Add text selection info
    if (includeTextSelection) {
      const selection = window.getSelection();
      if (selection && selection.rangeCount > 0) {
        const range = selection.getRangeAt(0);
        if (!range.collapsed) {
          try {
            const startContainer = range.startContainer.nodeType === 3 
              ? range.startContainer.parentElement 
              : range.startContainer;
            const endContainer = range.endContainer.nodeType === 3 
              ? range.endContainer.parentElement 
              : range.endContainer;
            if (startContainer && endContainer) {
              const cloneStart = getCloneElement(startContainer, clone);
              const cloneEnd = getCloneElement(endContainer, clone);
              if (cloneStart && cloneEnd) {
                cloneStart.setAttribute('data-selection-start', range.startOffset);
                cloneEnd.setAttribute('data-selection-end', range.endOffset);
                cloneStart.setAttribute('data-selection-text', selection.toString());
              }
            }
          } catch (e) { /* ignore */ }
        }
      }
    }

    // Add focus state
    if (includeKeyboardFocus && document.activeElement && document.activeElement !== document.body) {
      try {
        const cloneActive = getCloneElement(document.activeElement, clone);
        if (cloneActive) cloneActive.setAttribute('data-focused', 'true');
      } catch (e) { /* ignore */ }
    }

    // Add bounding box information for all elements
    if (includeClientRects) {
      document.querySelectorAll('*').forEach(el => {
        try {
          const rect = el.getBoundingClientRect();
          if (rect.width > 0 || rect.height > 0) {
            const cloneEl = getCloneElement(el, clone);
            if (!cloneEl) return;
            const bx = Math.round(rect.x);
            const by = Math.round(rect.y);
            const bw = Math.round(rect.width);
            const bh = Math.round(rect.height);
            cloneEl.setAttribute('data-bbox-x', bx);
            cloneEl.setAttribute('data-bbox-y', by);
            cloneEl.setAttribute('data-bbox-width', bw);
            cloneEl.setAttribute('data-bbox-height', bh);
            // Compute center and whether elementFromPoint hits this element
            const cx = rect.x + rect.width / 2;
            const cy = rect.y + rect.height / 2;
            let hit = false;
            try {
              const hitEl = document.elementFromPoint(cx, cy);
              hit = !!(hitEl && (hitEl === el || el.contains(hitEl)));
            } catch (e) {
              hit = false;
            }
            cloneEl.setAttribute('data-bbox-center-hit', hit ? 'true' : 'false');
          }
        } catch (e) { /* ignore */ }
      });
    }

    // === Augmented actions ===
    // Helpers to merge or attach actions to the corresponding cloned element
    function appendActionToCloneEl(cloneEl, item) {
      try {
        const prev = cloneEl.getAttribute('data-actions');
        let arr = [];
        if (prev) {
          try {
            const parsed = JSON.parse(prev);
            if (Array.isArray(parsed)) arr = parsed;
          } catch (e) { /* ignore malformed */ }
        }
        if (item && typeof item.user === 'string' && typeof item.assistant === 'string') {
          arr.push({ user: item.user, assistant: item.assistant });
          cloneEl.setAttribute('data-actions', JSON.stringify(arr));
        }
      } catch (e) { /* ignore */ }
    }

    // Synthetic: "Drag to select '{random text}'" -> "drag(from_coord=[x,y], to_coord=[x,y])"
    if (includeRandomTextActions) {
      const textEls = Array.from(document.querySelectorAll('*')).filter(el => {
        // heuristic: elements has no children
        if (el.children.length > 0) return false;
        // heuristic: element has text nodes and visible bbox
        // heuristic: element has at least 4 characters
        const hasText = (el.textContent || '').trim().length >= 4;
        if (!hasText) return false;
        const rect = el.getBoundingClientRect();
        return (rect.width > 0 && rect.height > 0);
      });
      for (let i = 0; i < textEls.length; i++) {
        const el = textEls[i];
        const pts = getTextDragPoints(el);
        if (pts && pts.success && typeof pts.text === 'string' && pts.text.trim() && pts.text.trim().length >= 4) {
          const user = `Drag to select '${pts.text.trim()}'`;
          // Keep absolute pixels; processors may normalize later
          const assistant = `drag(from_coord=[${Math.round(pts.point1.x)},${Math.round(pts.point1.y)}], to_coord=[${Math.round(pts.point2.x)},${Math.round(pts.point2.y)}])`;
          const cloneEl = getCloneElement(el, clone);
          if (cloneEl) appendActionToCloneEl(cloneEl, { user, assistant });
        }
      }
    }

    if (includeWindowManagementActions) {
      // Synthetic: "Close/Minimize/Maximize this window" -> "click" @ [data-focused="true"]
      //            "Close/Minimize/Maximize {window title}" -> "click"
      const labels = ["Close", "Maximize", "Minimize"];
      const selector = labels.map(l => `.title-bar-controls button[aria-label="${l}"]`).join(",");
      document.querySelectorAll(selector).forEach(btn => {
        // Nearest window container
        const win = btn.closest('.window');
        if (!win) return;
        if (win.getAttribute('data-focused') !== 'true') return;
        // Skip disabled
        const ariaDisabled = btn.getAttribute('aria-disabled');
        if (btn.disabled || ariaDisabled === 'true') return;
        // Ensure hit test at center is the button itself
        const rect = btn.getBoundingClientRect();
        if (!(rect.width > 0 && rect.height > 0)) return;
        const cx = rect.left + rect.width / 2;
        const cy = rect.top + rect.height / 2;
        let hitOk = false;
        try {
          const hitEl = document.elementFromPoint(cx, cy);
          hitOk = !!(hitEl && (hitEl === btn));
        } catch (e) { hitOk = false; }
        if (!hitOk) return;

        // Map aria-label to verb
        const aria = String(btn.getAttribute('aria-label') || '').trim();
        if (!labels.includes(aria)) return;
        const verb = aria; // Close/Maximize/Minimize

        // Prepare assistant click using absolute pixels (downstream will normalize)
        const assistant = `click(x=${Math.round(cx)}, y=${Math.round(cy)})`;
        const cloneBtn = getCloneElement(btn, clone);
        if (!cloneBtn) return;

        // Variant 1: "<Verb> this window"
        const user1 = `${verb} this window`;
        appendActionToCloneEl(cloneBtn, { user: user1, assistant });

        // Variant 2: "<Verb> {window title}" using .title-bar-text
        const titleEl = win.querySelector('.title-bar-text');
        const title = (titleEl && titleEl.textContent) ? titleEl.textContent.trim() : '';
        if (title) {
          const user2 = `${verb} ${title}`;
          appendActionToCloneEl(cloneBtn, { user: user2, assistant });
        }
      });

      // Synthetic: "Move to the {corner/edge} of this window frame" -> move_mouse
      //            "Move to the {corner/edge} of {window title}"
      // Only for focused windows
      document.querySelectorAll('.window[data-focused="true"]').forEach(win => {
        const rect = win.getBoundingClientRect();
        if (!(rect.width > 0 && rect.height > 0)) return;
        const left = rect.left;
        const right = rect.right;
        const top = rect.top;
        const bottom = rect.bottom;
        const midX = left + rect.width / 2;
        const midY = top + rect.height / 2;
        const points = [
          { name: 'top left', x: left, y: top },
          { name: 'top', x: midX, y: top },
          { name: 'top right', x: right - 1, y: top },
          { name: 'left', x: left, y: midY },
          { name: 'right', x: right - 1, y: midY },
          { name: 'bottom left', x: left, y: bottom - 1 },
          { name: 'bottom right', x: right - 1, y: bottom - 1 },
          { name: 'bottom', x: midX, y: bottom - 1 },
        ];
        const cloneWin = getCloneElement(win, clone);
        if (!cloneWin) return;
        const titleEl = win.querySelector('.title-bar-text');
        const title = (titleEl && titleEl.textContent) ? titleEl.textContent.trim() : '';
        for (const pt of points) {
          let hitOk = false;
          try {
            const hitEl = document.elementFromPoint(pt.x, pt.y);
            hitOk = !!(hitEl && (hitEl === win));
          } catch (e) { hitOk = false; }
          if (!hitOk) continue;
          const assistant = `move_mouse(x=${Math.round(pt.x)}, y=${Math.round(pt.y)})`;
          // this window frame
          appendActionToCloneEl(cloneWin, { user: `Move to the ${pt.name} of this window frame`, assistant });
          // title variant
          if (title) {
            appendActionToCloneEl(cloneWin, { user: `Move to the ${pt.name} of ${title}`, assistant });
          }
        }
      });

      // Synthetic: "Switch to {window title}" -> "click" @ .window:not([data-focused="true"]) .title-bar-text
      document.querySelectorAll('.window:not([data-focused="true"]) .title-bar-text').forEach(el => {
        const title = el.textContent.trim();
        const cloneEl = getCloneElement(el, clone);
        if (!cloneEl) return;
        const assistant = `click(x=${Math.round(el.getBoundingClientRect().left)}, y=${Math.round(el.getBoundingClientRect().top)})`;
        appendActionToCloneEl(cloneEl, { user: `Switch to ${title}`, assistant });
      });
    }

    // Return serialized HTML
    return clone.outerHTML;
  }

  window.__td_build_snapshot = buildSnapshot;
})();