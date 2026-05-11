use crate::types::events::GameEvent;
use crate::types::game_state::{GameState, WaitingFor};

use super::engine::{begin_pending_trigger_target_selection, check_exile_returns, EngineError};
use super::match_flow;
use super::players;
use super::sba;
use super::triggers;

pub(super) fn run_post_action_pipeline(
    state: &mut GameState,
    events: &mut Vec<GameEvent>,
    default_wf: &WaitingFor,
    skip_trigger_scan: bool,
) -> Result<WaitingFor, EngineError> {
    // Capture stack depth before any trigger/SBA processing so we can detect
    // whether new triggered abilities were added during this pipeline pass.
    let stack_before = state.stack.len();

    // CR 614.12a + CR 707.9: If the resolving event left the engine waiting on
    // a mid-entry choice (currently `CopyTargetChoice`), the entering object's
    // characteristics and granted triggers aren't finalized yet — trigger
    // scanning of its battlefield-entry event must wait until the choice
    // resolves and layers re-evaluate. Clone matching events into
    // `state.deferred_entry_events` (the `events` vec itself is preserved so
    // the frontend still sees the entry); `handle_copy_target_choice` replays
    // them through `process_triggers` after `BecomeCopy` resolves.
    let mid_entry_source = capture_mid_entry_deferred_events(state, events);

    // CR 603.2: Triggered abilities trigger at the moment the event occurs.
    // Scan for triggers BEFORE SBAs so that objects still on the battlefield
    // (e.g., a creature that just took lethal damage) are found by the scan.
    // This follows the same pattern as process_combat_damage_triggers in combat_damage.rs.
    if !skip_trigger_scan {
        let filtered_events: Vec<_> = events
            .iter()
            .filter(|event| !matches!(event, GameEvent::PhaseChanged { .. }))
            .filter(|event| !is_deferred_entry_event(mid_entry_source, event))
            .cloned()
            .collect();
        triggers::process_triggers(state, &filtered_events);
    }

    // CR 704.3: SBA/trigger loop. SBAs may generate events (e.g., ZoneChanged for
    // dying creatures) that need trigger processing. Repeat until no new SBAs fire,
    // matching the loop pattern in process_combat_damage_triggers.
    loop {
        let events_before = events.len();
        sba::check_state_based_actions(state, events);
        if events.len() > events_before {
            let sba_events: Vec<_> = events[events_before..].to_vec();
            triggers::process_triggers(state, &sba_events);
        } else {
            break;
        }
    }

    if !matches!(state.waiting_for, WaitingFor::Priority { .. }) {
        if matches!(state.waiting_for, WaitingFor::GameOver { .. }) {
            match_flow::handle_game_over_transition(state);
        }
        return Ok(state.waiting_for.clone());
    }

    // CR 800.4: If SBAs eliminated the player who was about to receive priority,
    // respect the reassignment that eliminate_player() already performed.
    if let Some(player) = default_wf.acting_player() {
        if !players::is_alive(state, player) {
            return Ok(state.waiting_for.clone());
        }
    }

    check_exile_returns(state, events);

    let delayed_events = triggers::check_delayed_triggers(state, events);
    events.extend(delayed_events);

    // CR 603.8: Check state triggers after event-based triggers.
    // State triggers fire when a condition is true, checked whenever a player
    // would receive priority.
    triggers::check_state_triggers(state);

    if let Some(waiting_for) = begin_pending_trigger_target_selection(state)? {
        state.waiting_for = waiting_for.clone();
        return Ok(waiting_for);
    }

    if state.stack.len() > stack_before {
        return Ok(flush_pending_miracle_offer(
            state,
            WaitingFor::Priority {
                player: state.active_player,
            },
        ));
    }

    if state.layers_dirty {
        super::layers::evaluate_layers(state);
    }

    Ok(flush_pending_miracle_offer(state, default_wf.clone()))
}

/// CR 702.94a + CR 603.11: Intercept a `WaitingFor::Priority` and replace it
/// with the head of `pending_miracle_offers` as `WaitingFor::MiracleReveal`,
/// dropping the queued offer so a subsequent Priority flush picks up the next
/// one (or returns the original Priority if the queue is empty).
///
/// Pass-through for any non-Priority `WaitingFor`: miracle prompts only
/// interrupt the normal priority window, not nested choices (mana payment,
/// target selection, etc.) that must complete before priority is granted.
///
/// Stale-offer filtering: offers whose `object_id` is no longer in the offer
/// player's hand (moved/exiled/destroyed between queue time and flush) are
/// discarded without prompting — the reveal is offered "as you draw it" per
/// CR 702.94a, and the card can no longer be revealed from hand.
fn flush_pending_miracle_offer(state: &mut GameState, outgoing: WaitingFor) -> WaitingFor {
    if !matches!(outgoing, WaitingFor::Priority { .. }) {
        return outgoing;
    }
    // `pop_next_live_miracle_offer` already drains stale entries internally,
    // so a single pop is sufficient here. Consume the offer regardless of the
    // player's eventual accept/decline so the queue progresses even if the
    // same spell's resolution queued multiple offers for the same player.
    match pop_next_live_miracle_offer(state) {
        Some(offer) => WaitingFor::MiracleReveal {
            player: offer.player,
            object_id: offer.object_id,
            cost: offer.cost,
        },
        None => outgoing,
    }
}

/// CR 614.12a + CR 707.9: If the post-action waiting state is a mid-entry
/// player choice (`CopyTargetChoice`), clone the entering object's
/// battlefield-entry `ZoneChanged` event into `state.deferred_entry_events`
/// and return the source id so `process_triggers` can filter it out. The
/// `events` vec itself is preserved so the frontend animates the entry as
/// soon as the spell resolves. `handle_copy_target_choice` drains
/// `deferred_entry_events` and replays them through `process_triggers` after
/// `BecomeCopy` resolves + layers re-evaluate, so granted ETBs (Callidus
/// Assassin's destroy-same-name) and observer ETBs (Soul Warden) both
/// match against the fully-realized copy.
fn capture_mid_entry_deferred_events(
    state: &mut GameState,
    events: &[GameEvent],
) -> Option<crate::types::identifiers::ObjectId> {
    let WaitingFor::CopyTargetChoice { source_id, .. } = state.waiting_for else {
        return None;
    };
    // Defense in depth: a prior `CopyTargetChoice` that exited abnormally
    // (concede mid-choice, eliminate_player, error return before drain) may
    // have left stale events for an unrelated source. Reset before capturing
    // the new entry's events so the replay in `handle_copy_target_choice`
    // never fires triggers against a phantom object.
    state.deferred_entry_events.clear();
    for event in events {
        if is_battlefield_entry_for(source_id, event) {
            state.deferred_entry_events.push(event.clone());
        }
    }
    Some(source_id)
}

fn is_deferred_entry_event(
    source: Option<crate::types::identifiers::ObjectId>,
    event: &GameEvent,
) -> bool {
    source.is_some_and(|src| is_battlefield_entry_for(src, event))
}

fn is_battlefield_entry_for(
    source: crate::types::identifiers::ObjectId,
    event: &GameEvent,
) -> bool {
    matches!(
        event,
        GameEvent::ZoneChanged { object_id, to, .. }
            if *object_id == source && *to == crate::types::zones::Zone::Battlefield
    )
}

/// Pop the next `MiracleOffer` whose `object_id` is still in the player's
/// hand. Stale offers (card left the hand) are discarded. Returns `None`
/// when the queue is empty or contains only stale entries.
fn pop_next_live_miracle_offer(
    state: &mut GameState,
) -> Option<crate::types::game_state::MiracleOffer> {
    while !state.pending_miracle_offers.is_empty() {
        let offer = state.pending_miracle_offers.remove(0);
        let still_in_hand = state.objects.get(&offer.object_id).is_some_and(|obj| {
            obj.zone == crate::types::zones::Zone::Hand && obj.owner == offer.player
        });
        if still_in_hand {
            return Some(offer);
        }
    }
    None
}
