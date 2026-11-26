<script setup lang="ts">
import { CircleHelp } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle, SheetTrigger } from '@/components/ui/sheet'

// 0.x's "Understanding Card Stats" sheet, one section per column. Its dashes
// are rewritten as commas, colons and full stops.
type Section = { title: string; paragraphs: string[]; example?: string }

const SECTIONS: Section[] = [
  {
    title: 'Kept %',
    paragraphs: [
      'The percentage of games where this card appeared in your opening hand and was kept (not mulliganed away). The number in brackets is the raw count of games kept.',
    ],
    example: '23% (12) means the card was kept in 12 out of ~52 possible games.',
  },
  {
    title: 'Kept Win %',
    paragraphs: [
      'Your win rate in games where this card was kept in your opening hand. The number in brackets is the sample size (total games where the card was kept).',
    ],
    example:
      '38% (8) means you won 38% of the 8 games where this card was kept. A high Kept Win % suggests the card is strong in openers.',
  },
  {
    title: 'Cast %',
    paragraphs: [
      'The percentage of games where this card was actually cast (put on the stack). The number in brackets is the raw count of games where the card was cast.',
      'If a card was cast via an alternative cost or casting method (flashback, madness, evoke, warp, free, dash, escape, ...), the breakdown appears below the cast count.',
    ],
    example:
      "27% (14) means you cast the card in 14 games. A low Cast % on a mainboard card may indicate it's hard to cast or frequently sided out.",
  },
  {
    title: 'Cast Win %',
    paragraphs: [
      "Your win rate in games where this card was cast. The number in brackets is the sample size. This shows correlation, not causation: a low Cast Win % doesn't necessarily mean the card is bad, you might only cast it when behind.",
    ],
    example:
      '75% (4) means you won 3 out of 4 games where the card was cast. Look for cards with decent sample sizes (5+) to draw meaningful conclusions.',
  },
  {
    title: 'Played %',
    paragraphs: [
      'The percentage of games where a land card was played (put onto the battlefield from hand). This is the land equivalent of Cast %: lands are "played", not "cast". The number in brackets is the raw count.',
    ],
  },
  {
    title: 'Kicked',
    paragraphs: [
      'The total number of times this card was cast with kicker across all games. Compare with total casts to see how often you have enough mana to kick it.',
    ],
  },
  {
    title: 'Activated',
    paragraphs: [
      "The total number of times this card's activated ability was used across all games. High activation counts indicate the card is sticking on the battlefield and generating value.",
    ],
  },
  {
    title: 'Pregame %',
    paragraphs: [
      'The percentage of games where this card triggered before Turn 1: either revealed from your opening hand (Chancellor cycle, Devourer of Destiny) or put onto the battlefield pre-game (Gemstone Caverns, Leylines). The number in brackets is the raw count.',
      'When a card has both revealed and played triggers recorded, hover the cell for the breakdown.',
    ],
  },
  {
    title: 'Pregame Win %',
    paragraphs: [
      "Your win rate in games where this card triggered pre-game. Compare against Kept Win % to see whether the opening-hand effect actually correlates with winning: a Leyline that’s 75% Pregame Win but 45% Cast Win tells you the card is mostly a dead draw unless you open on it.",
    ],
  },
  {
    title: 'Seen %',
    paragraphs: [
      'The percentage of games where this card left your library, whether drawn naturally, tutored, milled, or exiled. Any card that appeared in your hand, on the battlefield, in the graveyard, in exile, or on the stack counts as "seen." The number in brackets is the raw count of games seen.',
      "Matches imported from MTGO's game history have only MTGO's game log, which never shows your hand, so a card there counts as seen only once it was cast or played. When such games are in the current filters, Seen % and Seen Win % are marked est. and a note above the table says how many.",
    ],
    example:
      '54% (28) means the card was seen in 28 games. Seen % is usually higher than Cast % since it includes cards drawn but never cast.',
  },
  {
    title: 'Seen Win %',
    paragraphs: [
      'Your win rate in games where this card was seen (drawn or otherwise left the library). The number in brackets is the sample size.',
    ],
    example:
      '40% (10) means you won 4 out of 10 games where this card was seen. Compare Seen Win % with Cast Win %: a big gap may indicate the card is only good when you can actually cast it.',
  },
  {
    title: 'SB Out %',
    paragraphs: [
      'The percentage of postboard games (games 2 and 3) where this card was sided out of your deck. The number in brackets is the count of games it was removed. Only applies to postboard games.',
    ],
    example:
      "50% (10) means the card was sided out in 10 postboard games. A high SB Out % on a mainboard card may suggest it's a frequent sideboard cut in your meta.",
  },
  {
    title: 'SB In %',
    paragraphs: [
      'The percentage of postboard games (games 2 and 3) where this card was sided into your deck from the sideboard. The number in brackets is the count of games it was brought in. Only applies to postboard games.',
      "In imported matches with only MTGO's game log, sideboarding is visible only for cards that show up in the log, so SB In % and SB Out % can be low or missing for those games and are marked est.",
    ],
    example:
      "75% (6) means the card was sided in for 6 postboard games. A high SB In % on a sideboard card shows it's frequently relevant in your meta.",
  },
  {
    title: 'Games',
    paragraphs: [
      'The total number of games played with this card in the deck. This is the denominator for most percentage calculations. Cards with low game counts will have less reliable statistics.',
    ],
  },
]
</script>

<template>
  <Sheet>
    <SheetTrigger as-child>
      <Button
        variant="ghost"
        size="sm"
        class="bevel gap-1.5 border border-black/60 px-2.5 py-4 text-xs text-muted-foreground"
      >
        <CircleHelp class="size-3.5" />
        <span class="hidden lg:inline">Help</span>
      </Button>
    </SheetTrigger>
    <SheetContent side="right" class="overflow-y-auto sm:max-w-md">
      <SheetHeader>
        <SheetTitle>Understanding Card Stats</SheetTitle>
        <SheetDescription>How to read the metrics on this page and what they mean for your deck.</SheetDescription>
      </SheetHeader>
      <div class="flex flex-col gap-6 px-4 pb-6">
        <section>
          <h3 class="mb-1 text-sm font-semibold">How Win Rates Are Calculated</h3>
          <p class="text-sm text-muted-foreground">
            Win % columns are <span class="font-medium">adjusted for sample size</span>. A card with only a few games
            gets pulled toward your deck's overall win rate, so a lucky 2-0 doesn't show as a misleading 100%. The more
            games a card has, the closer its displayed value is to the raw win rate. Hover any win % to see the raw
            numbers behind it.
          </p>
          <p class="mt-1 text-sm text-muted-foreground">
            The <span class="font-medium">Trust</span> slider controls how many games a card needs before its own
            results outweigh the deck baseline. Set it to 0 to see raw win rates; raise it to be more skeptical of small
            samples.
          </p>
          <p class="mt-1 text-sm text-muted-foreground">
            Cards with no recorded games for a column show <span class="font-mono">-</span> and sort below cards with
            data.
          </p>
          <div class="mt-2 rounded-md bg-muted px-3 py-2 text-xs">
            <span class="font-medium">Example:</span> a card that went 2-0 shows around
            <span class="font-mono">52%</span> rather than <span class="font-mono">100%</span> when your deck wins 48%
            overall: two games isn't enough evidence to stray far from the baseline.
          </div>
        </section>

        <section v-for="section in SECTIONS" :key="section.title">
          <h3 class="mb-1 text-sm font-semibold">{{ section.title }}</h3>
          <p v-for="(text, index) in section.paragraphs" :key="index" class="text-sm text-muted-foreground" :class="{ 'mt-1': index > 0 }">
            {{ text }}
          </p>
          <div v-if="section.example" class="mt-2 rounded-md bg-muted px-3 py-2 text-xs">
            <span class="font-medium">Example:</span> {{ section.example }}
          </div>
        </section>

        <section>
          <h3 class="mb-1 text-sm font-semibold">Reading Win Rate Colors</h3>
          <p class="text-sm text-muted-foreground">
            Win rate values are color-coded: <span class="font-medium text-success">green</span> for win rates above
            55%, <span class="font-medium text-destructive">red</span> for win rates below 45%, and neutral for values in
            between. These thresholds help you quickly spot over- and under-performers.
          </p>
        </section>
      </div>
    </SheetContent>
  </Sheet>
</template>
