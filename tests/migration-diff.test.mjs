import test from "node:test";
import assert from "node:assert/strict";

import { testing } from "../src/lib/migration-diff.mjs";

const { splitPutStatements, groupPutStatements, extractGroupKey, resolvePreambles, parseHasClauses, diffEntityGroup } = testing;

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
  assert.equal(groups[0].variable, "module");
  assert.equal(groups[0].statements.length, 1);
  assert.equal(groups[1].variable, "r1");
  assert.equal(groups[1].statements.length, 2);
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

test("parseHasClauses extracts attribute-value pairs", () => {
  const stmt = 'put $r1 isa SchemaResource,\n  has docKey "key1",\n  has typeLabel "Type1",\n  has scopeNote "some note";';
  const clauses = parseHasClauses(stmt);
  assert.equal(clauses.length, 3);
  assert.equal(clauses[0].attribute, "docKey");
  assert.equal(clauses[0].value, '"key1"');
  assert.equal(clauses[1].attribute, "typeLabel");
  assert.equal(clauses[2].attribute, "scopeNote");
  assert.equal(clauses[2].value, '"some note"');
});

test("diffEntityGroup generates match/delete/insert for changed scopeNote", () => {
  const oldGroup = {
    variable: "r1",
    type: "SchemaResource",
    statements: [
      'put $r1 isa SchemaResource,\n  has docKey "key1",\n  has typeLabel "Type1",\n  has scopeNote "old note";',
      "put (resource: $r1, module: $module) isa inModule;",
    ],
  };
  const newGroup = {
    variable: "r1",
    type: "SchemaResource",
    statements: [
      'put $r1 isa SchemaResource,\n  has docKey "key1",\n  has typeLabel "Type1",\n  has scopeNote "new note";',
      "put (resource: $r1, module: $module) isa inModule;",
    ],
  };

  const result = diffEntityGroup(oldGroup, newGroup);
  assert.ok(result, "should produce an update statement");
  assert.ok(result.includes("match"));
  assert.ok(result.includes('has docKey "key1"'));
  assert.ok(result.includes("has scopeNote $r1_old_scopeNote"));
  assert.ok(result.includes("delete"));
  assert.ok(result.includes("has $r1_old_scopeNote of $r1"));
  assert.ok(result.includes("insert"));
  assert.ok(result.includes('"new note"'));
  // Should NOT include typeLabel in the update
  assert.ok(!result.includes("typeLabel"));
});

test("diffEntityGroup returns null when only relation puts changed", () => {
  const oldGroup = {
    variable: "r1",
    type: "SchemaResource",
    statements: [
      'put $r1 isa SchemaResource,\n  has docKey "key1",\n  has scopeNote "same";',
      "put (resource: $r1, module: $old_module) isa inModule;",
    ],
  };
  const newGroup = {
    variable: "r1",
    type: "SchemaResource",
    statements: [
      'put $r1 isa SchemaResource,\n  has docKey "key1",\n  has scopeNote "same";',
      "put (resource: $r1, module: $new_module) isa inModule;",
    ],
  };

  const result = diffEntityGroup(oldGroup, newGroup);
  assert.equal(result, null);
});

test("full diff flow: changed entity produces update, new entity produces put", () => {
  const oldText = `
put $module isa SchemaModule,
  has moduleKey "test",
  has moduleName "test";

put $r1 isa SchemaResource,
  has docKey "key1",
  has scopeNote "old note";
put (resource: $r1, module: $module) isa inModule;
`;

  const newText = `
put $module isa SchemaModule,
  has moduleKey "test",
  has moduleName "test";

put $r1 isa SchemaResource,
  has docKey "key1",
  has scopeNote "new note";
put (resource: $r1, module: $module) isa inModule;

put $r3 isa SchemaResource,
  has docKey "key3",
  has scopeNote "brand new";
put (resource: $r3, module: $module) isa inModule;
`;

  const oldGroups = groupPutStatements(splitPutStatements(oldText));
  const newGroups = groupPutStatements(splitPutStatements(newText));

  const oldMap = new Map();
  for (const g of oldGroups) oldMap.set(extractGroupKey(g), g);

  const newPuts = [];
  const updates = [];
  for (const g of newGroups) {
    const key = extractGroupKey(g);
    const oldGroup = oldMap.get(key);
    if (!oldGroup) {
      newPuts.push(g);
    } else if (oldGroup.statements.join("\n") !== g.statements.join("\n")) {
      const update = diffEntityGroup(oldGroup, g);
      if (update) updates.push(update);
    }
  }

  // $r1 changed → update statement
  assert.equal(updates.length, 1);
  assert.ok(updates[0].includes('has docKey "key1"'));
  assert.ok(updates[0].includes('"new note"'));

  // $r3 is new → put statement
  assert.equal(newPuts.length, 1);
  assert.equal(newPuts[0].variable, "r3");
});
