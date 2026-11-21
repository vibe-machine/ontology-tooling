# gist relations and their roles

Source: `ontology-gist/schema/gistCore.tql` (package gist 5.1.0). Since gist 5 the roles have
plain names (`isCategorizedBy` relates `categorizedThing` and `category`); the older
`<relation>Subject` / `<relation>Object` names are gone. Check the file before writing a role:
`rg -n -A3 '^relation <name>' ../ontology-gist/schema/gistCore.tql`.

Specialise a gist relation with roles named for your domain, mapped with `as`:

```typeql
relation readingSubject sub isBasedOn,
  relates pass as derivedThing,
  relates sourceRead as basis;
```

Role names must be unique across every relation that specialises the same gist relation, in every
installed package (see `typedb-limits-and-traps.md`).

## The ones packages use most

| Relation | Roles | Reads as | Use for |
|---|---|---|---|
| `isCategorizedBy` | `categorizedThing`, `category` | X is categorised by C | every fixed-list value |
| `isAbout` | `content`, `subjectMatter` | content is about X | records, notes, documentation |
| `isBasedOn` | `derivedThing`, `basis` | X is derived from Y | readings of a source, decisions from evidence |
| `isPartOf` | `part`, `whole` | X is part of Y | passages in a source, items in a document |
| `isMemberOf` | `member`, `collection` | X belongs to a group | membership |
| `hasParticipant` | `participationContext`, `participant` | someone took part | who read, who decided |
| `hasGoal` | `directedThing`, `goal` | X is done for Y | purpose of an activity |
| `precedes` | `predecessor`, `successor` | X comes before Y | order, revisions, blocking |
| `refersTo` | `referringThing`, `referent` | X mentions Y | references |
| `requires` | `requiringThing`, `requirement` | X needs Y | dependencies |
| `isProducedBy` | `product`, `producer` | X was made by Y | authorship, output |
| `isExpressedIn` | `expressedThing`, `language` | X is written in L | formats, languages |
| `hasAddress` | `addressedThing`, `address` | X is found at A | locations of copies |
| `isIdentifiedBy` | `identifiedThing`, `identifier` | X has identifier I | external ids |
| `conformsTo` | `conformingThing`, `standard` | X follows standard S | |
| `isGovernedBy` | `governedThing`, `governingAuthority` | X is governed by Y | |

## All relations

| Relation | Roles |
|---|---|
| `allows` | `permission`, `allowedActivity` |
| `comesFromAgent` (sub `hasParticipant`) | `transfer` as `participationContext`, `originatingAgent` as `participant` |
| `comesFromPlace` | `thing`, `originPlace` |
| `conformsTo` | `conformingThing`, `standard` |
| `contributesTo` | `contributorPart`, `contributedSystem` |
| `goesToAgent` (sub `hasParticipant`) | `transfer` as `participationContext`, `receivingAgent` as `participant` |
| `goesToPlace` | `thing`, `destinationPlace` |
| `hasAccuracy` | `measuredMagnitude`, `accuracyMagnitude` |
| `hasAddend` | `sumAspect`, `addend` |
| `hasAddress` | `addressedThing`, `address` |
| `hasAspect` | `magnitude`, `aspect` |
| `hasBiologicalParent` | `child`, `biologicalParent` |
| `hasBroader` | `narrowerThing`, `broaderThing` |
| `hasDirectBroader` (sub `hasBroader`) | `directBroaderThing` as `broaderThing` |
| `hasDivisor` | `dividendThing`, `divisor` |
| `hasGiver` (sub `hasParticipant`) | `exchange` as `participationContext`, `giver` as `participant` |
| `hasGoal` | `directedThing`, `goal` |
| `hasIncumbent` | `position`, `incumbent` |
| `hasMagnitude` | `measuredThing`, `magnitude` |
| `hasMultiplier` | `factoredThing`, `multiplier` |
| `hasNavigationalParent` | `childCategory`, `parentCategory` |
| `hasParticipant` | `participationContext`, `participant` |
| `hasParty` (sub `hasParticipant`) | `partyContext` as `participationContext`, `party` as `participant` |
| `hasPhysicalLocation` | `locatedThing`, `physicalLocation` |
| `hasRecipient` (sub `hasParticipant`) | `exchange` as `participationContext`, `recipient` as `participant` |
| `hasSubtrahend` | `differenceAspect`, `subtrahend` |
| `hasUniqueBroader` (sub `hasBroader`) | `uniqueBroaderThing` as `broaderThing` |
| `hasUniqueNavigationalParent` (sub `hasNavigationalParent`) | `uniqueParentCategory` as `parentCategory` |
| `hasUnitGroup` | `aspect`, `unitGroup` |
| `hasUnitOfMeasure` | `magnitude`, `unitOfMeasure` |
| `isAbout` | `content`, `subjectMatter` |
| `isAffectedBy` | `affectedThing`, `effectSource` |
| `isAllocatedBy` | `allocatedThing`, `allocator` |
| `isAssignmentOf` | `assignment`, `assignedResource` |
| `isAssignmentTo` | `assignment`, `assignmentTarget` |
| `isBasedOn` | `derivedThing`, `basis` |
| `isCategorizedBy` | `categorizedThing`, `category` |
| `isConnectedTo` | `endpoint` |
| `isDirectPartOf` (sub `isPartOf`) | inherits `part`, `whole` |
| `isExpressedIn` | `expressedThing`, `language` |
| `isFirstMemberOf` (sub `isMemberOf`) | `firstMember` as `member`, `orderedCollection` as `collection` |
| `isGeoContainedIn` | `containedPlace`, `containingPlace` |
| `isGovernedBy` | `governedThing`, `governingAuthority` |
| `isIdentifiedBy` | `identifiedThing`, `identifier` |
| `isMadeUpOf` | `madeThing`, `substance` |
| `isMemberOf` | `member`, `collection` |
| `isPartOf` | `part`, `whole` |
| `isProducedBy` | `product`, `producer` |
| `isRecognizedBy` | `recognizedThing`, `recognizingParty` |
| `isRenderedOn` | `content`, `renderingMedium` |
| `isTriggeredBy` | `triggeredThing`, `triggeringEvent` |
| `isUnderJurisdictionOf` | `jurisdictionalThing`, `jurisdiction` |
| `linksRel` | `networkLink`, `networkNode` |
| `linksFrom` (sub `linksRel`) | `originNode` as `networkNode` |
| `linksTo` (sub `linksRel`) | `destinationNode` as `networkNode` |
| `occursIn` | `occurrence`, `place` |
| `offersToProvide` | `offer`, `providedThing` |
| `offersToReceive` | `offer`, `expectedThing` |
| `ownsRel` | `owner`, `ownedThing` |
| `precedes` | `predecessor`, `successor` |
| `precedesDirectly` (sub `precedes`) | inherits `predecessor`, `successor` |
| `prevents` | `intention`, `prohibitedBehavior` |
| `providesOrderFor` | `orderedMember`, `orderedItem` |
| `refersTo` | `referringThing`, `referent` |
| `requires` | `requiringThing`, `requirement` |
