<script>
  import { menu, closeMenu } from './menu.js';
  import { report } from './stores.js';
  import { fold } from './format.js';

  const SUBMENU_SWITCH_MS = 250;
  const TYPEAHEAD_RESET_MS = 500;
  const DRAG_THRESHOLD = 4;

  let root;
  let active = -1;
  let sub = -1;
  let subActive = -1;
  let switchTimer = null;
  let typed = '';
  let typedAt = 0;
  let opened = null;
  let pressedSinceOpen = false;

  $: if ($menu !== opened) reset($menu);
  $: items = $menu ? $menu.items : [];
  $: subItems = sub === -1 ? [] : items[sub].submenu;

  function reset(next) {
    opened = next;
    pressedSinceOpen = false;
    active = -1;
    sub = -1;
    subActive = -1;
    typed = '';
    clearTimeout(switchTimer);
  }

  const selectable = (item) => !item.sep && !item.title && !item.disabled;

  function placeRoot(node, point) {
    const { offsetWidth: width, offsetHeight: height } = node;
    const left = point.x + width > innerWidth ? point.x - width : point.x;
    const top = point.y + height > innerHeight ? point.y - height : point.y;
    node.style.left = `${Math.max(0, Math.min(left, innerWidth - width))}px`;
    node.style.top = `${Math.max(0, Math.min(top, innerHeight - height))}px`;
    node.focus();
  }

  function placeSub(node, index) {
    const parent = root.querySelector(`[data-index="${index}"]`).getBoundingClientRect();
    const { offsetWidth: width, offsetHeight: height } = node;
    const left = parent.right - 2 + width > innerWidth ? parent.left + 2 - width : parent.right - 2;
    const top = parent.top - 3 + height > innerHeight ? parent.bottom + 3 - height : parent.top - 3;
    node.style.left = `${Math.max(0, left)}px`;
    node.style.top = `${Math.max(0, top)}px`;
  }

  function close() {
    const { returnFocus } = $menu;
    closeMenu();
    returnFocus.focus();
  }

  async function run(item) {
    if (!selectable(item)) return;
    close();
    try {
      await item.action();
    } catch (error) {
      console.error(error);
      report.error(`${item.label} failed: ${error.message}`);
    }
  }

  function step(list, from, direction, wrap) {
    for (let offset = 1; offset <= list.length; offset++) {
      const index = from + offset * direction;
      if (!wrap && (index < 0 || index >= list.length)) return from;
      const wrapped = (index + list.length) % list.length;
      if (selectable(list[wrapped])) return wrapped;
    }
    return from;
  }

  function edge(list, direction) {
    return step(list, direction === 1 ? -1 : list.length, direction, false);
  }

  function openSub(index, withKeyboard) {
    clearTimeout(switchTimer);
    active = index;
    sub = index;
    subActive = withKeyboard ? edge(items[index].submenu, 1) : -1;
  }

  function hoverRoot(event, index) {
    const item = items[index];
    if (!selectable(item) || index === sub) {
      if (index === sub) {
        clearTimeout(switchTimer);
        active = index;
        subActive = -1;
      }
      return;
    }
    const commit = () => {
      active = index;
      subActive = -1;
      if (item.submenu) openSub(index, false);
      else sub = -1;
    };
    clearTimeout(switchTimer);
    if (sub !== -1 && event.movementX !== 0) switchTimer = setTimeout(commit, SUBMENU_SWITCH_MS);
    else commit();
  }

  function hoverSub(index) {
    clearTimeout(switchTimer);
    active = sub;
    if (selectable(subItems[index])) subActive = index;
  }

  function clickRoot(index) {
    const item = items[index];
    if (item.submenu && selectable(item)) openSub(index, false);
    else run(item);
  }

  function typeTo(character) {
    const now = Date.now();
    typed = now - typedAt > TYPEAHEAD_RESET_MS ? character : typed + character;
    typedAt = now;
    const inSub = sub !== -1 && subActive !== -1;
    const list = inSub ? subItems : items;
    const from = (inSub ? subActive : active) + (typed.length === 1 ? 1 : 0);
    for (let offset = 0; offset < list.length; offset++) {
      const index = (from + offset + list.length) % list.length;
      if (selectable(list[index]) && fold(list[index].label).startsWith(fold(typed))) {
        if (inSub) subActive = index;
        else {
          active = index;
          sub = -1;
        }
        return;
      }
    }
  }

  function keydown(event) {
    event.stopPropagation();
    const inSub = sub !== -1 && subActive !== -1;
    const list = inSub ? subItems : items;
    const at = inSub ? subActive : active;
    const set = (index) => {
      if (inSub) subActive = index;
      else {
        active = index;
        sub = -1;
      }
    };
    const current = at === -1 ? null : list[at];
    if (event.key === 'ArrowDown') set(at === -1 ? edge(list, 1) : step(list, at, 1, !event.repeat));
    else if (event.key === 'ArrowUp') set(at === -1 ? edge(list, -1) : step(list, at, -1, !event.repeat));
    else if (event.key === 'Home') set(edge(list, 1));
    else if (event.key === 'End') set(edge(list, -1));
    else if (event.key === 'ArrowLeft' && inSub) {
      sub = -1;
      subActive = -1;
    } else if (event.key === 'ArrowRight' && !inSub && current?.submenu) openSub(at, true);
    else if (event.key === 'Enter' || event.key === ' ') {
      if (!current) return event.preventDefault();
      if (current.submenu) openSub(at, true);
      else run(current);
    } else if (event.key === 'Escape') {
      if (sub !== -1) {
        sub = -1;
        subActive = -1;
      } else close();
    } else if (event.key === 'Tab') close();
    else if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
      typeTo(event.key);
    } else return;
    event.preventDefault();
  }

  function swallowNextClick() {
    const swallow = (event) => {
      event.stopPropagation();
      event.preventDefault();
    };
    window.addEventListener('click', swallow, { capture: true, once: true });
    window.addEventListener(
      'pointerup',
      () => setTimeout(() => window.removeEventListener('click', swallow, { capture: true })),
      { capture: true, once: true },
    );
  }

  function outsideDown(event) {
    if (!$menu) return;
    pressedSinceOpen = true;
    if (event.target.closest('.menu')) return;
    if (event.button === 0) {
      swallowNextClick();
      close();
    } else closeMenu();
  }

  function release(event) {
    if (!$menu || event.button !== 2 || pressedSinceOpen) return;
    if (Math.hypot(event.clientX - $menu.x, event.clientY - $menu.y) <= DRAG_THRESHOLD) return;
    const target = event.target.closest('.menu .item');
    if (!target) return;
    const list = target.closest('.submenu') ? subItems : items;
    const item = list[Number(target.dataset.index)];
    if (!item.submenu) run(item);
  }
</script>

<svelte:window on:pointerdown|capture={outsideDown} on:mouseup={release} on:blur={closeMenu} />

{#if $menu}
  {#key $menu}
    <div
      class="menu"
      role="menu"
      tabindex="-1"
      bind:this={root}
      use:placeRoot={$menu}
      on:keydown={keydown}
      on:contextmenu|preventDefault|stopPropagation
    >
      {#each items as item, index}
        {#if item.sep}
          <div class="sep"></div>
        {:else if item.title}
          <div class="title">{item.title}</div>
        {:else}
          <div
            class="item"
            class:active={active === index}
            class:disabled={item.disabled}
            class:primary={item.primary}
            role={item.checked === undefined ? 'menuitem' : 'menuitemcheckbox'}
            aria-checked={item.checked}
            aria-disabled={item.disabled}
            data-index={index}
            on:pointermove={(e) => hoverRoot(e, index)}
            on:click={() => clickRoot(index)}
          >
            <span class="check">{item.checked ? '✓' : ''}</span>
            <span class="label">{item.label}</span>
            <span class="hint">{item.submenu ? '▸' : (item.hint ?? '')}</span>
          </div>
        {/if}
      {/each}
    </div>
    {#if sub !== -1}
      {#key sub}
        <div class="menu submenu" role="menu" use:placeSub={sub} on:contextmenu|preventDefault|stopPropagation>
          {#each subItems as item, index}
            {#if item.sep}
              <div class="sep"></div>
            {:else if item.title}
              <div class="title">{item.title}</div>
            {:else}
              <div
                class="item"
                class:active={subActive === index}
                class:disabled={item.disabled}
                class:primary={item.primary}
                role={item.checked === undefined ? 'menuitem' : 'menuitemcheckbox'}
                aria-checked={item.checked}
                aria-disabled={item.disabled}
                data-index={index}
                on:pointermove={() => hoverSub(index)}
                on:click={() => run(item)}
              >
                <span class="check">{item.checked ? '✓' : ''}</span>
                <span class="label">{item.label}</span>
                <span class="hint">{item.hint ?? ''}</span>
              </div>
            {/if}
          {/each}
        </div>
      {/key}
    {/if}
  {/key}
{/if}
