import { expect, test } from 'vitest';
import { deriveTurns, frameAtPosition, frameForTimestamp, hasNumberedTurns, notableFrames, timelinePositions, timestampMs } from './replayTimeline';

type Card = { Id: number; CatalogID: number; Zone: string; Owner: number };

function frame(cards: Card[], lives: [number, number] = [20, 20], counts: [number, number] = [7, 7]) {
    return {
        timestamp: '',
        content: {
            Players: [
                { Id: 1, Name: 'Hero', Life: lives[0], HandCount: counts[0] },
                { Id: 2, Name: 'Villain', Life: lives[1], HandCount: counts[1] },
            ],
            Cards: cards,
        },
    };
}

const card = (Id: number, Zone: string): Card => ({ Id, CatalogID: Id, Zone, Owner: 1 });

test('a card reaching the stack or battlefield is notable', () => {
    const frames = [frame([card(1, 'Hand')]), frame([card(2, 'Stack')]), frame([card(3, 'Battlefield')])];

    expect(notableFrames(frames)).toEqual([1, 2]);
});

test('a permanent leaving the battlefield is notable', () => {
    const frames = [frame([card(3, 'Battlefield')]), frame([card(9, 'Graveyard')])];

    expect(notableFrames(frames)).toEqual([1]);
});

test('a life change is notable', () => {
    const frames = [frame([]), frame([], [20, 17])];

    expect(notableFrames(frames)).toEqual([1]);
});

test('hand and library churn alone is not', () => {
    const frames = [frame([card(1, 'Hand')]), frame([card(1, 'Hand'), card(4, 'Hand')], [20, 20], [8, 7]), frame([card(3, 'Battlefield')]), frame([card(3, 'Battlefield')])];

    expect(notableFrames(frames)).toEqual([2]);
});

test('turns count only when at least one is numbered', () => {
    expect(hasNumberedTurns([{ number: null, from: 0, to: 10, player: null }])).toBe(false);
    expect(hasNumberedTurns([{ number: null, from: 0, to: 2, player: null }, { number: 1, from: 2, to: 10, player: 1 }])).toBe(true);
});

test('clock and ISO timestamps both read as milliseconds into the day', () => {
    expect(timestampMs('20:47:33')).toBe(74853000);
    expect(timestampMs('20:47:33.250')).toBe(74853250);
    expect(timestampMs('2026-09-19T20:47:33Z')).toBe(74853000);
    expect(timestampMs('2026-09-19T20:47:33.250+00:00')).toBe(74853250);
});

const at = (timestamp: string) => ({ ...frame([]), timestamp });

test('frames sit on the timeline by when they happened, thinking included', () => {
    expect(timelinePositions([at('00:00:00'), at('00:00:01'), at('00:00:04')])).toEqual([0, 25, 100, 100]);
});

test('frames without timestamps are spaced evenly', () => {
    expect(timelinePositions([at(''), at(''), at('')])).toEqual([0, 50, 100, 100]);
});

test('a position picks the frame showing at that moment', () => {
    const positions = [0, 25, 100, 100];

    expect(frameAtPosition(positions, 0)).toBe(0);
    expect(frameAtPosition(positions, 24)).toBe(0);
    expect(frameAtPosition(positions, 60)).toBe(1);
    expect(frameAtPosition(positions, 100)).toBe(2);
    expect(frameAtPosition(positions, -5)).toBe(0);
});

function turnFrame(active: number | undefined, step?: string) {
    return { timestamp: '', content: { Players: [{ Id: 1, Name: 'Hero' }, { Id: 2, Name: 'Villain' }], Cards: [], ActivePlayer: active, Step: step } };
}

test('turns follow the active player once it changes', () => {
    const frames = [turnFrame(2), turnFrame(2, 'Upkeep'), turnFrame(2, 'PreCombatMain'), turnFrame(1, 'Upkeep'), turnFrame(2, 'Upkeep')];

    expect(deriveTurns(frames, [])).toEqual([
        { number: null, from: 0, to: 1, player: null },
        { number: 1, from: 1, to: 3, player: 2 },
        { number: 2, from: 3, to: 4, player: 1 },
        { number: 3, from: 4, to: 5, player: 2 },
    ]);
});

test('an active player that never changes falls back to the log', () => {
    const frames = [turnFrame(1, 'Upkeep'), turnFrame(1, 'EndOfTurn'), turnFrame(1, 'Upkeep')];
    frames.forEach((frame, i) => (frame.timestamp = `12:00:0${i * 5}`));

    const turns = deriveTurns(frames, [
        { timestamp: '12:00:00', message: 'Turn 1:' },
        { timestamp: '12:00:10', message: 'Turn 1:' },
    ]);

    expect(turns.map((turn) => [turn.from, turn.player])).toEqual([
            [0, 1],
            [2, 2],
        ]);
});

test('log_lines_pair_with_hh_mm_ss_frames', () => {
    const at = (timestamp: string) => ({ timestamp, content: { Players: [], Cards: [] } });
    const frames = [at('20:47:33'), at('20:47:35.250'), at('20:47:40')];

    expect(frameForTimestamp(frames, '20:47:33')).toBe(0);
    expect(frameForTimestamp(frames, '20:47:34')).toBe(1);
    expect(frameForTimestamp(frames, '20:47:35')).toBe(1);
    expect(frameForTimestamp(frames, '20:47:36')).toBe(2);
    expect(frameForTimestamp(frames, '20:48:00')).toBe(2);
});
