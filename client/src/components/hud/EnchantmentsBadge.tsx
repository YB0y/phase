import type { PlayerId } from "../../adapter/types.ts";
import { useCardHover } from "../../hooks/useCardHover.ts";
import { useGameStore } from "../../stores/gameStore.ts";
import { useUiStore } from "../../stores/uiStore.ts";

interface Props {
  playerId: PlayerId;
}

const STABLE_EMPTY: readonly never[] = [];

/**
 * Trailing-row HUD badge that surfaces player-attached Auras (Curse cycle,
 * Faith's Fetters, Dictate of Kruphix, etc.) without disturbing the plate
 * layout. Slots into the same row as Monarch/Initiative/Counter badges
 * because, semantically, "this player is enchanted" belongs to the same
 * vocabulary of imposed-state indicators.
 *
 * Reads `gameState.derived.auras_attached_to_player`, an engine-authored
 * projection (see `crates/engine/src/game/derived_views.rs`). Per CLAUDE.md
 * the FE never scans the battlefield for `attached_to.type === "Player"` —
 * that's game logic owned by the engine.
 *
 * Interactions:
 *   - Hover: previews the first Aura via `useCardHover`. Auto-injects the
 *     `data-card-hover` data attribute that `usePreviewDismiss` polls for —
 *     without it the preview clears in ~300ms (useCardHover.ts:42-47).
 *   - Click: dispatches `setEnchantmentsDialogPlayer(playerId)` to open the
 *     dialog. The dialog itself is rendered by `<PlayerEnchantmentsDialog>`
 *     mounted inside `<DialogHost>` (GamePage), NOT here. The badge lives
 *     inside HudPlate, which sets a Tailwind `transform` CSS property and
 *     becomes a containing block for any `fixed inset-0` descendants — a
 *     dialog rendered as a child of the badge would shrink to HudPlate's
 *     bounding box. See DialogHost.tsx:113-122 for the contract.
 */
export function EnchantmentsBadge({ playerId }: Props) {
  const auraIds = useGameStore(
    (s) => s.gameState?.derived?.auras_attached_to_player?.[String(playerId)] ?? STABLE_EMPTY,
  );
  const setEnchantmentsDialogPlayer = useUiStore((s) => s.setEnchantmentsDialogPlayer);

  const previewId = auraIds[0] ?? null;
  const { handlers, firedRef } = useCardHover(previewId);

  if (auraIds.length === 0) return null;

  const count = auraIds.length;
  const ariaLabel =
    count === 1 ? "1 enchantment on this player" : `${count} enchantments on this player`;
  const tooltip =
    count === 1
      ? "Hover to preview, click to view"
      : `${count} enchantments — click to view all`;

  return (
    <button
      {...handlers}
      type="button"
      aria-label={ariaLabel}
      title={tooltip}
      onClick={() => {
        if (firedRef.current) {
          firedRef.current = false;
          return;
        }
        setEnchantmentsDialogPlayer(playerId);
      }}
      className="relative inline-flex h-6 min-w-6 shrink-0 cursor-pointer items-center justify-center gap-0.5 rounded-full px-1.5 text-[11px] font-bold leading-none text-violet-50 ring-1 ring-violet-300/60 bg-gradient-to-b from-violet-500 to-violet-700 shadow-[0_0_12px_rgba(139,92,246,0.45)] transition-all duration-150 hover:from-violet-400 hover:to-violet-600 hover:shadow-[0_0_18px_rgba(167,139,250,0.7)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-violet-200"
    >
      <span aria-hidden className="text-[13px] leading-none">✧</span>
      {count > 1 ? <span className="tabular-nums">×{count}</span> : null}
    </button>
  );
}
