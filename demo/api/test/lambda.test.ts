import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { stripApiPrefix } from "../src/lambda.js";

describe("stripApiPrefix", () => {
  it("strips /api/<region> from CloudFront paths", () => {
    assert.equal(stripApiPrefix("/api/us-west-2/posts"), "/posts");
    assert.equal(stripApiPrefix("/api/us-east-1/posts/abc/comments"), "/posts/abc/comments");
    assert.equal(stripApiPrefix("/api/us-west-2/"), "/");
    assert.equal(stripApiPrefix("/api/us-west-2"), "/");
  });
  it("leaves direct paths alone", () => {
    assert.equal(stripApiPrefix("/posts"), "/posts");
    assert.equal(stripApiPrefix("/apix/posts"), "/apix/posts");
    assert.equal(stripApiPrefix("/"), "/");
  });
});
