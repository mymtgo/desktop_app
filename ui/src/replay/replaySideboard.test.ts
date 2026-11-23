import { expect, test } from 'vitest';
import { gameSideboard, sideboardAtStart, sideboardChanges, sideboardCaptions } from './replaySideboard';
import type { ReplayCard, ReplayFrame } from './types';

const LOCAL = 1;
const OPPONENT = 2;

let nextId = 100;
const card = (name: string, Zone = 'Sideboard', Owner = LOCAL, extra: Partial<ReplayCard> = {}): ReplayCard => ({
    Id: nextId++,
    CatalogID: name.length,
    Zone,
    Owner,
    name,
    ...extra,
});

const frame = (Cards: ReplayCard[]): ReplayFrame => ({
    timestamp: '12:00:00',
    content: {
        Players: [
            { Id: OPPONENT, Name: 'Opponent', IsLocal: false },
            { Id: LOCAL, Name: 'you', IsLocal: true },
        ],
        Cards,
    },
});

const names = (cards: ReplayCard[] | null) => cards?.map((item) => item.name).sort();
const lines = (changes: ReturnType<typeof sideboardChanges>) => ({
    in: changes.in.map((line) => `${line.count} ${line.name}`),
    out: changes.out.map((line) => `${line.count} ${line.name}`),
});

test('the starting sideboard is your sideboard in the first frame that has one', () => {
    const frames = [
        frame([]),
        frame([card('Wear // Tear'), card('Mystical Dispute'), card('Duress', 'Sideboard', OPPONENT), card('Ragavan', 'Hand')]),
        frame([card('Mystical Dispute')]),
    ];

    expect(names(sideboardAtStart(frames))).toEqual(['Mystical Dispute', 'Wear // Tear']);
});

test('a companion shown outside the sideboard still starts in it', () => {
    const frames = [frame([card('Lurrus', 'Companion', LOCAL, { ActualZone: 'Sideboard' }), card('Duress')])];

    expect(names(sideboardAtStart(frames))).toEqual(['Duress', 'Lurrus']);
});

test('a game with no sideboard recorded has no starting sideboard', () => {
    expect(sideboardAtStart([frame([card('Ragavan', 'Hand')])])).toBe(null);
    expect(sideboardAtStart([])).toBe(null);
});

test('cards leaving the sideboard came in and cards joining it went out', () => {
    const before = [card('Wear // Tear'), card('Wear // Tear'), card('Mystical Dispute')];
    const after = [card('Mystical Dispute'), card('Ragavan'), card('Thoughtseize')];

    expect(lines(sideboardChanges(before, after))).toEqual({
        in: ['2 Wear // Tear'],
        out: ['1 Ragavan', '1 Thoughtseize'],
    });
});

test('a card split between deck and sideboard counts only the copies that moved', () => {
    // One copy in each; the main deck copy comes out, so the sideboard now holds two.
    const before = [card('Surgical Extraction'), card('Duress')];
    const after = [card('Surgical Extraction'), card('Surgical Extraction')];

    expect(lines(sideboardChanges(before, after))).toEqual({
        in: ['1 Duress'],
        out: ['1 Surgical Extraction'],
    });
});

test('an unchanged sideboard has no changes', () => {
    expect(lines(sideboardChanges([card('Duress')], [card('Duress')]))).toEqual({ in: [], out: [] });
});

test('cards with no known name are matched by catalog id', () => {
    const before = [card('', 'Sideboard', LOCAL, { name: null, CatalogID: 7 })];
    const after = [card('', 'Sideboard', LOCAL, { name: null, CatalogID: 9 })];

    const changes = sideboardChanges(before, after);

    expect([changes.in.map((line) => line.card.CatalogID), changes.out.map((line) => line.card.CatalogID)]).toEqual([[7], [9]]);
});

test('only as many sideboard copies as went out are marked', () => {
    const current = [card('Surgical Extraction'), card('Surgical Extraction'), card('Duress')];
    const changes = sideboardChanges([card('Surgical Extraction'), card('Duress')], current);

    const captions = sideboardCaptions(current, changes);

    expect([...captions.values()]).toEqual(['Taken out']);
    expect(current.filter((item) => captions.has(item.Id)).map((item) => item.name).join()).toBe('Surgical Extraction');
});

test('a recorded sideboard is used over the frames, one card per copy', () => {
    const frames = [frame([card('Duress')])];
    const entries = [
        { catalog_id: 7, quantity: 2, name: 'Wear // Tear', type: 'Instant', image: null },
        { catalog_id: 9, quantity: 1, name: 'Abrade', type: 'Instant', image: 'https://cards.scryfall.io/a.jpg' },
    ];

    const cards = gameSideboard(frames, entries);

    expect(names(cards)).toEqual(['Abrade', 'Wear // Tear', 'Wear // Tear']);
    expect(new Set(cards?.map((item) => item.Id)).size).toBe(3);
    expect(cards?.every((item) => item.Zone === 'Sideboard')).toBeTruthy();
});

test('without a recorded sideboard the frames are used', () => {
    expect(names(gameSideboard([frame([card('Duress')])], null))).toEqual(['Duress']);
    expect(names(gameSideboard([frame([card('Duress')])], []))).toEqual(['Duress']);
    expect(gameSideboard([frame([])], undefined)).toBe(null);
});
