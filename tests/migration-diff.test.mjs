import test from "node:test";
import assert from "node:assert/strict";

import { testing } from "../src/lib/migration-diff.mjs";

const { splitPutStatements, groupPutStatements, extractGroupKey, resolvePreambles } = testing;

test("splitPutStatements parses multi-line put statements", () => {
  const text = `
# comment
put $r1 isa SchemaResource,
  has docKey "key1",
  has typeLabel "Type1";
put (resource: $r1, module: $module) isa inModule;

put $r2 isa SchemaResource,
  has docKey "key2";
`;
  const stmts = splitPutStatements(text);
  assert.equal(stmts.length, 3);
  assert.ok(stmts[0].startsWith("put $r1 isa SchemaResource,"));
  assert.ok(stmts[0].includes('has docKey "key1"'));
  assert.ok(stmts[1].startsWith("put (resource: $r1"));
  assert.ok(stmts[2].startsWith("put $r2 isa SchemaResource,"));
});

test("splitPutStatements handles empty input", () => {
  assert.deepEqual(splitPutStatements(""), []);
  assert.deepEqual(splitPutStatements("# just comments\n# more"), []);
});

test("groupPutStatements groups entity with its relation", () => {
  const stmts = [
    'put $module isa SchemaModule,\n  has moduleKey "vibemachine";',
    'put $r1 isa SchemaResource,\n  has docKey "key1";',
    "put (resource: $r1, module: $module) isa inModule;",
    'put $r2 isa SchemaResource,\n  has docKey "key2";',
    "put (resource: $r2, module: $module) isa inModule;",
  ];
  const groups = groupPutStatements(stmts);
  assert.equal(groups.length, 3);

  // $module group — standalone entity
  assert.equal(groups[0].variable, "module");
  assert.equal(groups[0].statements.length, 1);

  // $r1 group — entity + inModule relation
  assert.equal(groups[1].variable, "r1");
  assert.equal(groups[1].statements.length, 2);

  // $r2 group — entity + inModule relation
  assert.equal(groups[2].variable, "r2");
  assert.equal(groups[2].statements.length, 2);
});

test("extractGroupKey uses type and first has attribute", () => {
  const group = {
    variable: "r1",
    type: "SchemaResource",
    statements: ['put $r1 isa SchemaResource,\n  has docKey "https://example.com#Foo",\n  has typeLabel "Foo";'],
  };
  assert.equal(extractGroupKey(group), 'SchemaResource::docKey::https://example.com#Foo');
});

test("extractGroupKey falls back to raw prefix for keyless statements", () => {
  const group = {
    variable: null,
    type: null,
    statements: ["put (resource: $r1, module: $module) isa inModule;"],
  };
  const key = extractGroupKey(group);
  assert.ok(key.startsWith("raw::"));
});

test("resolvePreambles includes referenced but undefined variables", () => {
  const moduleGroup = {
    variable: "module",
    type: "SchemaModule",
    statements: ['put $module isa SchemaModule, has moduleKey "test";'],
  };
  const changedGroup = {
    variable: "r1",
    type: "SchemaResource",
    statements: [
      'put $r1 isa SchemaResource, has docKey "key1";',
      "put (resource: $r1, module: $module) isa inModule;",
    ],
  };

  const preambles = resolvePreambles([changedGroup], [moduleGroup, changedGroup]);
  assert.equal(preambles.length, 1);
  assert.equal(preambles[0].variable, "module");
});

test("resolvePreambles returns empty when all variables are self-contained", () => {
  const group = {
    variable: "draft",
    type: "SpecificationStatus",
    statements: ['put $draft isa SpecificationStatus, has status_label "draft";'],
  };
  const preambles = resolvePreambles([group], [group]);
  assert.equal(preambles.length, 0);
});

test("full diff flow: changed scopeNote produces correct diff", () => {
  const oldText = `
put $module isa SchemaModule,
  has moduleKey "test",
  has moduleName "test";

put $r1 isa SchemaResource,
  has docKey "key1",
  has scopeNote "old note";
put (resource: $r1, module: $module) isa inModule;

put $r2 isa SchemaResource,
  has docKey "key2",
  has scopeNote "unchanged";
put (resource: $r2, module: $module) isa inModule;
`;

  const newText = `
put $module isa SchemaModule,
  has moduleKey "test",
  has moduleName "test";

put $r1 isa SchemaResource,
  has docKey "key1",
  has scopeNote "new note";
put (resource: $r1, module: $module) isa inModule;

put $r2 isa SchemaResource,
  has docKey "key2",
  has scopeNote "unchanged";
put (resource: $r2, module: $module) isa inModule;
`;

  const oldGroups = groupPutStatements(splitPutStatements(oldText));
  const newGroups = groupPutStatements(splitPutStatements(newText));

  const oldMap = new Map();
  for (const g of oldGroups) oldMap.set(extractGroupKey(g), g.statements.join("\n"));

  const changed = [];
  for (const g of newGroups) {
    const key = extractGroupKey(g);
    if (oldMap.get(key) !== g.statements.join("\n")) {
      changed.push(g);
    }
  }

  // Only $r1 changed (scopeNote updated)
  assert.equal(changed.length, 1);
  assert.equal(changed[0].variable, "r1");

  // Preamble resolution: $r1 references $module
  const preambles = resolvePreambles(changed, newGroups);
  assert.equal(preambles.length, 1);
  assert.equal(preambles[0].variable, "module");
});
