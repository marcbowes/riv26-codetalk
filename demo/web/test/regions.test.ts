import assert from "node:assert/strict";
import { test } from "node:test";
import { BOTH, pickRegions, withRegionParam } from "../src/regions.ts";

const regions = [
  { region: "us-west-2", apiUrl: "/api/us-west-2/" },
  { region: "us-east-1", apiUrl: "/api/us-east-1/" },
];

test("no param shows the first Region on its own", () => {
  assert.deepEqual(pickRegions(null, regions), { single: 0, both: false, fromParam: false });
});

test("a configured Region shows just that one", () => {
  assert.deepEqual(pickRegions("us-east-1", regions), { single: 1, both: false, fromParam: true });
  assert.deepEqual(pickRegions("us-west-2", regions), { single: 0, both: false, fromParam: true });
});

test("both shows every Region", () => {
  assert.deepEqual(pickRegions(BOTH, regions), { single: 0, both: true, fromParam: false });
});

test("an unknown Region falls back to the first", () => {
  assert.deepEqual(pickRegions("eu-west-1", regions), { single: 0, both: false, fromParam: false });
  assert.deepEqual(pickRegions("", regions), { single: 0, both: false, fromParam: false });
});

test("both needs more than one Region", () => {
  assert.deepEqual(pickRegions(BOTH, regions.slice(0, 1)), { single: 0, both: false, fromParam: false });
});

test("local dev: the duplicated local entries still resolve", () => {
  const local = [
    { region: "local", apiUrl: "http://localhost:3000" },
    { region: "local", apiUrl: "http://localhost:3000" },
  ];
  assert.deepEqual(pickRegions(null, local), { single: 0, both: false, fromParam: false });
  assert.deepEqual(pickRegions(BOTH, local), { single: 0, both: true, fromParam: false });
});

test("the URL keeps other params and the hash", () => {
  assert.equal(withRegionParam("https://x.example/?a=1#post", BOTH), "/?a=1&region=both#post");
  assert.equal(withRegionParam("https://x.example/?region=both&a=1", "us-east-1"), "/?region=us-east-1&a=1");
  assert.equal(withRegionParam("https://x.example/?region=both&a=1", undefined), "/?a=1");
  assert.equal(withRegionParam("https://x.example/?region=both", undefined), "/");
});
