# gist entity types and attributes

Source: `ontology-gist/schema/gistCore.tql` (package gist 5.1.0, translated from Semantic Arts
gist v14.0.0). Regenerate this page from that file when gist changes; do not trust it over the
file.

gist's types are not `@abstract`, so a type built on them cannot be abstract either.

## Choosing a supertype

| What the new type is | Build on | Example in a package |
|---|---|---|
| A document, a source, written content | `FormattedContent`, `ContentExpression`, `Text` | Lingo `EvidenceSource` |
| A message received or sent | `Message` | |
| A place in or part of content | `Content` with `isPartOf` | Lingo `Passage` |
| Work to be done | `Task`, `Project` | |
| A goal, requirement, statement of intent | `Specification`, `Requirement`, `Intention` | Lingo `ModelingBrief` |
| A decision or judgement | `Determination` | Lingo `Reconciliation`, `ClaimCheck` |
| Something that happened | `HistoricalEvent`, `Event` | Lingo `Reading` |
| A fixed list value | `Category` (or `Tag`) | Lingo `SourceKind` |
| A grouping | `Collection`, `OrderedCollection` | |
| A software or other system | `System`; a part of one: `Component` | |
| A scheduled milestone or release | `ScheduledEvent`, `ScheduledTask` | |
| A time-bound role binding | `Assignment` | |
| A reusable pattern or process | `Template`, `TaskTemplate` | |
| A concept or idea | `KnowledgeConcept` | |
| A person or organisation | gist's `Person`, `Organization` directly; do not subtype for a role | |

Check the package's own schema for the parent it actually chose before copying an example.

## Hierarchy

```
Thing
├── Aspect
├── Category
│   ├── AddressUsageType, Behavior, DegreeOfCommitment, Discipline,
│   ├── ElectronicAddressType, EquipmentType, GeneralMediaType, MediaType,
│   └── Medium, PhysicalActionType, PhysicalAddressType, ProductCategory, Tag
├── Component
│   └── NetworkLink, NetworkNode, OrderedMember
├── Composite
│   ├── Collection
│   │   ├── ControlledVocabulary
│   │   ├── OrderedCollection
│   │   │   └── GeoRoute
│   │   └── UnitGroup
│   ├── Network
│   └── System
├── Content
│   ├── Address
│   │   └── ElectronicAddress, PhysicalAddress
│   ├── ContentExpression
│   │   ├── FormattedContent
│   │   │   └── RenderedContent
│   │   ├── Message
│   │   └── Text
│   └── ID
├── Event
│   ├── ContemporaryEvent, ContingentEvent, Determination, HistoricalEvent,
│   ├── PhysicalEvent, Transaction
│   ├── ScheduledEvent
│   │   └── ScheduledTask
│   └── Task
│       └── Project
├── GeoLocation
│   └── GeoPoint, GeoVolume
├── GeoRegion
│   └── GovernedGeoRegion
│       └── CountryGeoRegion
├── IntellectualProperty
│   └── KnowledgeConcept
├── Intention
│   ├── Agreement
│   │   └── Account, Contract
│   ├── Commitment
│   │   └── ContingentObligation
│   │       └── Offer
│   ├── Function, Permission, Requirement, Restriction
│   └── Specification
│       ├── CatalogItem
│       │   └── BundledCatalogItem, ProductSpecification, ServiceSpecification
│       └── ContractTerm, EventSpecification
├── Language
├── Magnitude
│   └── ReferenceValue
├── Organization
│   ├── GovernmentOrganization
│   │   └── CountryGovernment, SubCountryGovernment
│   └── IntergovernmentalOrganization
├── PhysicalIdentifiableItem
│   ├── Equipment
│   ├── Landmark
│   │   └── Building
│   └── LivingThing
│       └── Person
├── PhysicalSubstance
├── SchemaMetaData
├── Template
│   └── TaskTemplate
├── TemporalRelation
│   └── Assignment
├── TimeInterval
└── UnitOfMeasure
```

## Attributes gist already declares

Reuse these; never declare an attribute of the same name.

- Text: `name`, `description`, `containedText` (with `encryptedText`, `uniqueText`), `idText`,
  `symbol`, `sourceIRI`
- Numbers: `numericValue`, `sequence` (integer), `conversionFactor`, `conversionOffset`,
  `latitude`, `longitude`, `exponentOf…` (unit dimensions)
- Time (all under `atDateTime`, value datetime): `startDateTime`, `endDateTime`,
  `actualStart…` / `actualEnd…` / `plannedStart…` / `plannedEnd…` (`DateTime`, `Date`, `Minute`,
  `Year`, and `Microsecond` for actual), `birthDate`, `deathDate`, `isRecordedAt`

Check the file before adding any attribute:
`rg -n '^attribute' ../ontology-gist/schema/gistCore.tql`.
