# Schema patterns for a package on gist

TypeQL 3 syntax is in the `typeql` skill; general design guidance in `modeling`. This page holds
the patterns ontology packages use. `ontology-lingo/schema/lingo.tql` is the reference package
written in this style; read it before writing a new one.

## File layout

```typeql
# <Package>: one paragraph on what the model records and which questions it answers.

define

# ── Values ──────────────────────────────────────────────
# name, description, containedText, sequence and atDateTime come from gist.
attribute sourceId value string;

# ── Fixed lists ─────────────────────────────────────────
# ── <Layer or area> ─────────────────────────────────────
```

Only declare attributes gist and the other installed packages do not already declare
(`references/gist-types.md`).

## Naming

| Element | Convention | Examples |
|---|---|---|
| Entity | PascalCase, a plain noun, no package prefix | `EvidenceSource`, `Passage` |
| Relation | lowerCamelCase noun or phrase | `readingSubject`, `placement` |
| Role | lowerCamelCase, specific enough to be unique in its gist family | `classifiedSource`, `sourceRead` |
| Attribute | lowerCamelCase | `sourceId`, `publishedYear` |
| Key | `<thing>Id` | `claimId`, `briefId` |

Names are global across every package in the database. Search the siblings first:
`rg -nw '<Name>' ../ontology-*/schema/*.tql`. (vibemachine prefixes its entities with `Vibe`;
follow the package's own convention when extending it.)

## Entity on a gist type

```typeql
entity EvidenceSource sub FormattedContent,
  owns sourceId @key,
  owns name,
  owns authorName,
  owns publishedYear,
  plays sourceClassification:classifiedSource,
  plays placement:passageSource;
```

Give every entity that data refers to a `@key`. Keys are how data files `put` records safely and
how upgrades match rows.

## Fixed list: a category per list

One category type per list, each with its own specialisation of `isCategorizedBy`:

```typeql
entity SourceKind sub Category, owns name @key, plays sourceClassification:sourceKind;

relation sourceClassification sub isCategorizedBy,
  relates classifiedSource as categorizedThing,
  relates sourceKind as category;
```

Seed values in a data file, keyed on `name`:

```typeql
put $book isa SourceKind, has name "book";
put $essay isa SourceKind, has name "essay";
```

Each list gets its own relation so a query reads plainly and role names stay unique in the
`isCategorizedBy` family.

## Specialising a gist relation

```typeql
relation readingSubject sub isBasedOn,
  relates pass as derivedThing,
  relates sourceRead as basis;

relation readership sub hasParticipant,
  relates reading as participationContext,
  relates reader as participant;
```

Use a relation without a gist parent only when no gist relation fits, and say why in a comment.

## A dependency's type in your relation

A type a dependency owns (gist's `Person`, a beads `Bead`) may play a role in a relation this
package declares, and nothing else:

```typeql
relation readership sub hasParticipant,
  relates reading as participationContext,
  relates reader as participant;

Person plays readership:reader;      # allowed: readership is ours
# Person owns readerNote;            # refused by the app: changes a type we do not own
```

## Parts and wholes

```typeql
relation placement sub isPartOf,
  relates passage as part,
  relates passageSource as whole;
```

## Revisions instead of edits

When a record must never be edited in place (a brief, a decision), model revisions: each revision
is its own entity, ordered with `precedes`, and lists what is in force. A change makes a new
revision.

## Data files

- Keyed records: `put` on the key, then `update` other attributes (safe to run twice).
- One variable per `put` statement; never reuse one across `put`s in a file.
- Keep each file within 50 blocks and 50,000 characters, or let `ont prepare-package` split it.

## Anti-patterns

1. Declaring an attribute gist or a sibling package already declares.
2. Using a raw gist relation instead of a specialisation with plain role names.
3. Generic role names (`subject`, `item`) that collide in a gist relation family.
4. `@values` string lists instead of category types.
5. `insert` for keyed seed data (fails or duplicates on reapply).
6. Adding `owns` or a parent to a dependency's type instead of using a subtype or your own relation.
