// One Regional Aurora DSQL cluster, half of a multi-Region pair.
//
// Peering through CloudFormation is two-phase (see deploy.sh):
//   1. create both clusters with only the witness Region; they wait in PENDING_SETUP
//   2. update both with MultiRegionProperties.Clusters = [both ARNs], concurrently
import * as cdk from "aws-cdk-lib";
import * as dsql from "aws-cdk-lib/aws-dsql";
import type { Construct } from "constructs";

export interface ClusterStackProps extends cdk.StackProps {
  witnessRegion: string;
  /** Both clusters' ARNs (this one included). Omit only in the create phase. */
  clusterArns?: string[];
}

export class ClusterStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: ClusterStackProps) {
    super(scope, id, props);

    const cluster = new dsql.CfnCluster(this, "Cluster", {
      deletionProtectionEnabled: true,
      multiRegionProperties: {
        witnessRegion: props.witnessRegion,
        ...(props.clusterArns ? { clusters: props.clusterArns } : {}),
      },
      tags: [{ key: "Name", value: `dat401-blog-${this.region}` }],
    });
    // Deletion protection guards the data; destroy.sh turns it off first.
    cluster.applyRemovalPolicy(cdk.RemovalPolicy.DESTROY);

    new cdk.CfnOutput(this, "ClusterId", { value: cluster.attrIdentifier });
    new cdk.CfnOutput(this, "ClusterArn", { value: cluster.attrResourceArn });
    new cdk.CfnOutput(this, "ClusterEndpoint", {
      // <identifier>.dsql.<region>.on.aws (computed so it resolves even while PENDING_SETUP)
      value: `${cluster.attrIdentifier}.dsql.${this.region}.on.aws`,
    });
    new cdk.CfnOutput(this, "Peered", { value: props.clusterArns ? "true" : "false" });
  }
}
