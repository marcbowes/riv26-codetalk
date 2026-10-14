# Step 1: second cluster, via AI

```
Create a new Aurora DSQL cluster in us-west-2 and tell me its endpoint.
```

Expected: the agent runs `aws dsql create-cluster` (or uses the MCP server)
and reports the identifier and endpoint. This is cluster B, the one that should
be cleaned up in step 10. Clusters show up as random IDs, so mixing them up is
believable.
