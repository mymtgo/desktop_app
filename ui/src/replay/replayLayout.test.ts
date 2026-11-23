import { expect, test } from 'vitest';
import { nextCardHeight, replayLayoutFor } from './replayLayout';

test('phones pick a compact layout by orientation', () => {
    expect(replayLayoutFor(390, 844)).toBe('compact-portrait');
    expect(replayLayoutFor(844, 390)).toBe('compact-landscape');
    expect(replayLayoutFor(600, 550)).toBe('compact-portrait');
});

test('tablets and desktops stay regular', () => {
    expect(replayLayoutFor(768, 1024)).toBe('regular');
    expect(replayLayoutFor(1024, 768)).toBe('regular');
    expect(replayLayoutFor(1440, 900)).toBe('regular');
});

test('spare room on the board is shared between its card rows', () => {
    expect(nextCardHeight(100, 120, 6, [56, 176])).toBe(120);
    expect(nextCardHeight(100, -60, 6, [56, 176])).toBe(90);
    expect(nextCardHeight(100, 5, 6, [56, 176])).toBe(100);
});

test('the card height stays within its bounds', () => {
    expect(nextCardHeight(170, 600, 6, [56, 176])).toBe(176);
    expect(nextCardHeight(60, -600, 6, [56, 176])).toBe(56);
    expect(nextCardHeight(100, 50, 0, [56, 176])).toBe(100);
});
