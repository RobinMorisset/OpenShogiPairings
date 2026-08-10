<script lang="ts">
  import { _ } from "svelte-i18n";
  import type { BracketMatch, Cup, CupBracketView, Player } from "../types";
  import { cupStageLabel } from "../pairingSource";

  interface Props {
    /** The server-derived bracket: structure + results, the authoritative shape. */
    bracket: CupBracketView;
    /** The frozen cup, for seed numbers next to the first-round players. */
    cup: Cup;
    /** All registered players, for tournament-number → name lookup. */
    players: Player[];
  }

  let { bracket, cup, players }: Props = $props();

  // Tournament number → player, for names.
  const byTid = $derived(
    new Map(
      players
        .filter((p): p is Player & { tournament_id: number } => p.tournament_id != null)
        .map((p) => [p.tournament_id, p]),
    ),
  );

  // Tournament number → 1-based seed (its position in the frozen seed order).
  const seedOf = $derived(new Map(cup.seed_order.map((tid, i) => [tid, i + 1])));

  // --- layout --------------------------------------------------------------
  //
  // A mirrored bracket: the top half of every round feeds the left side (rounds
  // running left→right), the bottom half feeds the right side (running
  // right→left), and the two semifinals meet in a single final in the middle.
  // Half the height of a one-sided bracket, at the cost of extra width.

  const BOX_W = 172;
  const ROW_H = 23;
  const BOX_H = ROW_H * 2;
  const COL_GAP = 54;
  const V_GAP = 16;
  const PAD = 16;
  const HEADER_H = 30;

  const top = PAD + HEADER_H;

  type PNode = {
    m: BracketMatch;
    x: number; // left edge
    cy: number; // centre y
    isFirstRound: boolean;
    isFinal: boolean;
  };

  // Child (right edge) → parent (left edge): a round's elbow connector.
  function elbow(child: { x: number; cy: number }, parent: { x: number; cy: number }): string {
    const x0 = child.x + BOX_W;
    const px = parent.x;
    const midX = (x0 + px) / 2;
    return `M ${x0} ${child.cy} H ${midX} V ${parent.cy} H ${px}`;
  }

  // A reconstructed bracket-tree node. The server emits each round's matches in
  // *fold* order, so match `k` of round `c` is fed by matches `k` and
  // `len-1-k` of round `c-1` (the outer fold) — not the adjacent pair.
  type TNode = {
    m: BracketMatch;
    c: number; // round index (0 = first round)
    x: number;
    cy: number;
    children?: [TNode, TNode];
  };

  const layout = $derived.by(() => {
    const rounds = bracket.rounds;
    const depth = rounds.length; // number of bracket rounds incl. the final
    const step = BOX_W + COL_GAP;
    // The qualifier format prepends a play-off column, so the bracket itself
    // starts one column further right.
    const qualification = bracket.qualification ?? null;
    const qualCols = qualification ? 1 : 0;
    const colXpos = (i: number) => PAD + (i + qualCols) * step;

    // Rebuild the tree top-down from the final, following the outer fold. The
    // first-round nodes are kept by index so the play-off matches can be hung off
    // the ones they feed.
    const firstRound: TNode[] = [];
    function build(c: number, k: number): TNode {
      const node: TNode = { m: rounds[c][k], c, x: 0, cy: 0 };
      if (c > 0) {
        const lenPrev = rounds[c - 1].length;
        node.children = [build(c - 1, k), build(c - 1, lenPrev - 1 - k)];
      } else {
        firstRound[k] = node;
      }
      return node;
    }
    const root = build(depth - 1, 0); // the final (depth ≥ 3, so it has children)

    // Vertical placement: an in-order walk lays the first round top→bottom; each
    // parent centres on its two children. Horizontal placement is by round.
    let slot = 0;
    function place(node: TNode) {
      node.x = colXpos(node.c);
      if (!node.children) {
        node.cy = top + slot++ * (BOX_H + V_GAP) + BOX_H / 2;
        return;
      }
      place(node.children[0]);
      place(node.children[1]);
      node.cy = (node.children[0].cy + node.children[1].cy) / 2;
    }
    place(root);

    // Flatten to render nodes + connectors. Each connector carries its child's
    // winner up to the parent (`flow`), so a player's path lights up on hover.
    const nodes: PNode[] = [];
    const links: { d: string; flow: number | null }[] = [];
    function walk(node: TNode) {
      nodes.push({
        m: node.m,
        x: node.x,
        cy: node.cy,
        isFirstRound: node.c === 0,
        isFinal: node.c === depth - 1,
      });
      if (node.children) {
        for (const ch of node.children) {
          links.push({ d: elbow(ch, node), flow: ch.m.winner });
          walk(ch);
        }
      }
    }
    walk(root);

    // The play-off column: match `j` feeds the *second* slot of first-round match
    // `len - 1 - j` (the fold), so each sits at its target's height and the
    // connector is a straight run into the box it feeds.
    if (qualification) {
      qualification.forEach((m, j) => {
        const target = firstRound[qualification.length - 1 - j];
        const node: PNode = { m, x: PAD, cy: target.cy, isFirstRound: true, isFinal: false };
        nodes.push(node);
        links.push({ d: elbow(node, target), flow: m.winner });
      });
    }

    // Column headers: the stage label above each round, the play-off first.
    const headers = rounds.map((col, c) => ({
      x: colXpos(c) + BOX_W / 2,
      label: cupStageLabel(col[0].stage, $_),
    }));
    if (qualification) {
      headers.unshift({
        x: PAD + BOX_W / 2,
        label: cupStageLabel(qualification[0].stage, $_),
      });
    }

    const bracketBottom = top + rounds[0].length * (BOX_H + V_GAP) - V_GAP;

    // The third-place match sits below the bracket, in the final's column.
    const smallFinalNode: PNode | null = bracket.small_final
      ? {
          m: bracket.small_final,
          x: colXpos(depth - 1),
          cy: bracketBottom + 44 + BOX_H / 2,
          isFirstRound: false,
          isFinal: false,
        }
      : null;

    const width = colXpos(depth - 1) + BOX_W + PAD;
    const height = (smallFinalNode ? smallFinalNode.cy + BOX_H / 2 : bracketBottom) + PAD;

    return { nodes, links, headers, smallFinalNode, width, height };
  });

  // Players knocked out of the bracket: the loser of every decided match. A
  // match winner who is *not* in this set is still in the cup (advancing, or the
  // champion); one who is here won that round but was eliminated later, so their
  // win is shown muted.
  const eliminated = $derived.by(() => {
    const out = new Set<number>();
    for (const round of [bracket.qualification ?? [], ...bracket.rounds]) {
      for (const m of round) {
        if (m.winner != null && m.player1 != null && m.player2 != null) {
          out.add(m.winner === m.player1 ? m.player2 : m.player1);
        }
      }
    }
    return out;
  });

  /** The visual tier of a slot: the champion (final winner), a win by a player
   *  still in the cup, a win by a since-eliminated player, or a plain slot. */
  function tierOf(n: PNode, player: number | null, isWinner: boolean): "champ" | "alive" | "faded" | "plain" {
    if (!isWinner || player == null) return "plain";
    if (n.isFinal) return "champ";
    return eliminated.has(player) ? "faded" : "alive";
  }

  /** A player's short label — last name plus first initial, trimmed to fit. */
  function nameOf(tid: number | null): string {
    if (tid == null) return $_("cupBracket.tbd");
    const p = byTid.get(tid);
    if (!p) return `#${tid}`;
    const initial = p.first_name ? ` ${p.first_name[0]}.` : "";
    const full = `${p.last_name}${initial}`;
    return full.length > 22 ? `${full.slice(0, 21)}…` : full;
  }

  // Hover-to-highlight: the tournament number under the cursor, resolved from the
  // `data-tid` of the slot beneath it (one delegated handler, so no per-row
  // listeners). A player's slots and the connectors that carried them light up.
  let hovered = $state<number | null>(null);
  function trackHover(event: MouseEvent) {
    const el = (event.target as HTMLElement | null)?.closest?.("[data-tid]") as HTMLElement | null;
    const tid = el?.dataset.tid;
    hovered = tid != null ? Number(tid) : null;
  }
</script>

{#snippet matchBox(n: PNode)}
  {@const m = n.m}
  {@const x = n.x}
  {@const y = n.cy - BOX_H / 2}
  {@const decided = m.winner != null}
  {@const won1 = decided && m.winner === m.player1}
  {@const won2 = decided && m.winner === m.player2}
  {@const t1 = tierOf(n, m.player1, won1)}
  {@const t2 = tierOf(n, m.player2, won2)}
  {@const on1 = m.player1 != null && m.player1 === hovered}
  {@const on2 = m.player2 != null && m.player2 === hovered}
  {@const seed1 = n.isFirstRound && m.player1 != null ? seedOf.get(m.player1) : undefined}
  {@const seed2 = n.isFirstRound && m.player2 != null ? seedOf.get(m.player2) : undefined}
  {@const nameX = x + (n.isFirstRound ? 30 : 10)}
  <g class="match">
    <rect class="box" class:final-won={n.isFinal && decided} {x} {y} width={BOX_W} height={BOX_H} rx="6" />
    <line class="divider" x1={x} y1={y + ROW_H} x2={x + BOX_W} y2={y + ROW_H} />
    <!-- top player -->
    <rect class="slot" class:won={t1 === "alive"} class:won-faded={t1 === "faded"} class:champ={t1 === "champ"}
          class:on-path={on1} data-tid={m.player1 ?? undefined} {x} {y} width={BOX_W} height={ROW_H} rx="6" />
    {#if seed1 != null}<text class="seed" x={x + 8} y={y + ROW_H / 2}>{seed1}</text>{/if}
    <text class="name" class:won={t1 === "alive" || t1 === "champ"} class:won-faded={t1 === "faded"} class:on-path={on1} x={nameX} y={y + ROW_H / 2}>{t1 === "champ" ? "🏆 " : ""}{nameOf(m.player1)}</text>
    <!-- bottom player -->
    <rect class="slot" class:won={t2 === "alive"} class:won-faded={t2 === "faded"} class:champ={t2 === "champ"}
          class:on-path={on2} data-tid={m.player2 ?? undefined} {x} y={y + ROW_H} width={BOX_W} height={ROW_H} rx="6" />
    {#if seed2 != null}<text class="seed" x={x + 8} y={y + ROW_H + ROW_H / 2}>{seed2}</text>{/if}
    <text class="name" class:won={t2 === "alive" || t2 === "champ"} class:won-faded={t2 === "faded"} class:on-path={on2} x={nameX} y={y + ROW_H + ROW_H / 2}>{t2 === "champ" ? "🏆 " : ""}{nameOf(m.player2)}</text>
  </g>
{/snippet}

<div class="cup-bracket">
  <svg viewBox="0 0 {layout.width} {layout.height}" width={layout.width} height={layout.height}
       role="img" aria-label={$_("cupBracket.title", { values: { size: bracket.size } })}
       onmousemove={trackHover} onmouseleave={() => (hovered = null)}>
    <!-- connectors (behind the boxes) -->
    {#each layout.links as l, i (i)}
      <path class="link" class:on-path={hovered != null && l.flow === hovered} d={l.d} />
    {/each}

    <!-- column headers -->
    {#each layout.headers as h, i (i)}
      <text class="stage" x={h.x} y={PAD + 14} text-anchor="middle">{h.label}</text>
    {/each}

    <!-- match boxes -->
    {#each layout.nodes as n, i (i)}
      {@render matchBox(n)}
    {/each}

    <!-- small final (3rd place) -->
    {#if layout.smallFinalNode}
      {@const sf = layout.smallFinalNode}
      <text class="stage" x={sf.x + BOX_W / 2} y={sf.cy - BOX_H / 2 - 8} text-anchor="middle">{$_("pairingSource.thirdPlace")}</text>
      {@render matchBox(sf)}
    {/if}
  </svg>
</div>

<style>
  .cup-bracket {
    overflow-x: auto;
    padding: 0.5rem 0 1rem;
  }
  svg {
    max-width: none; /* keep intrinsic size; the container scrolls if narrow */
    font-family: inherit;
  }
  .link {
    fill: none;
    stroke: var(--border);
    stroke-width: 1.5;
  }
  .link.on-path {
    stroke: var(--color-accent-strong);
    stroke-width: 2.5;
  }
  .box {
    fill: var(--bg-surface);
    stroke: var(--border);
    stroke-width: 1;
  }
  .box.final-won {
    stroke: var(--border-warning);
  }
  .divider {
    stroke: var(--border-divider);
    stroke-width: 1;
  }
  .slot {
    fill: transparent;
  }
  .slot.won {
    fill: var(--bg-accent);
    opacity: 0.16;
  }
  /* A win by a player later knocked out — a fainter tint than a live win. */
  .slot.won-faded {
    fill: var(--bg-accent);
    opacity: 0.06;
  }
  .slot.champ {
    fill: var(--bg-warning);
  }
  /* Hovered player's row — declared last so it overrides won/faded/champ fills. */
  .slot.on-path {
    fill: var(--bg-accent);
    opacity: 0.3;
  }
  .name {
    font-size: 12px;
    fill: var(--text);
    dominant-baseline: middle;
    pointer-events: none; /* let the slot rect beneath receive hover */
  }
  .name.won {
    font-weight: 700;
    fill: var(--text);
  }
  .name:not(.won):not(.won-faded) {
    fill: var(--text-secondary);
  }
  /* A since-eliminated player's win: full colour (still legible as a win) but
     not bold, so it reads as quieter than a live win. */
  .name.won-faded {
    fill: var(--text);
    font-weight: 400;
  }
  /* Declared after the above so it wins the specificity tie when a hovered
     player's row is highlighted. */
  .name.on-path {
    fill: var(--color-accent-strong);
    font-weight: 700;
  }
  .seed {
    font-size: 10px;
    fill: var(--text-tertiary);
    font-variant-numeric: tabular-nums;
    dominant-baseline: middle;
    pointer-events: none;
  }
  .stage {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.03em;
    fill: var(--text-secondary);
    text-transform: uppercase;
  }

  @media print {
    .link { stroke: #000; }
    .box, .divider { stroke: #000; fill: transparent; }
    .box.final-won { stroke: #000; }
    .name, .seed, .stage { fill: #000; }
    .slot.won { fill: #ddd; opacity: 1; }
    .slot.won-faded { fill: #f0f0f0; opacity: 1; }
    .slot.champ { fill: #f0d060; }
  }
</style>
