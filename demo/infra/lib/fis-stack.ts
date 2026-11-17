// An AWS FIS experiment template that takes one Region out of the blog: that
// Region's Aurora DSQL endpoint refuses every connection, and drops the ones
// already open (aws:dsql:cluster-connection-failure at 100%). Its Lambdas then
// answer 503, so clients (the web page, loadgen) move to the other Region.
//
// Measured 2026-11-17 on a scratch cluster: open connections are dropped about
// 20 s after start-experiment ("FATAL: internal error ... Ongoing AWS Fault
// Injection Service (FIS) experiment"), new ones are refused at once ("unable
// to accept connection"), and both work again as soon as it ends.
//
// The duration is a safety cap: the talk stops the experiment by hand.
import * as cdk from "aws-cdk-lib";
import * as fis from "aws-cdk-lib/aws-fis";
import * as iam from "aws-cdk-lib/aws-iam";
import type { Construct } from "constructs";
import { fisRoleName } from "./names";

export interface FisStackProps extends cdk.StackProps {
  /** The cluster to fail: this Region's half of the multi-Region cluster. */
  clusterArn: string;
}

export class FisStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: FisStackProps) {
    super(scope, id, props);

    const role = new iam.Role(this, "Role", {
      roleName: fisRoleName(this.region),
      description: `DAT401: lets AWS FIS fail connections to the ${this.region} DSQL cluster`,
      assumedBy: new iam.ServicePrincipal("fis.amazonaws.com", {
        conditions: { StringEquals: { "aws:SourceAccount": this.account } },
      }),
    });
    // InjectError has no resource type; scope it to this cluster with the FIS
    // condition key. ForAllValues alone would allow a request with no targets,
    // hence the Null check.
    role.addToPolicy(
      new iam.PolicyStatement({
        sid: "InjectConnectionFailures",
        actions: ["dsql:InjectError"],
        resources: ["*"],
        conditions: {
          "ForAllValues:ArnEquals": { "dsql:FisTargetArns": [props.clusterArn] },
          Null: { "dsql:FisTargetArns": "false" },
        },
      }),
    );
    role.addToPolicy(
      new iam.PolicyStatement({ sid: "ReadCluster", actions: ["dsql:GetCluster"], resources: [props.clusterArn] }),
    );
    role.addToPolicy(
      new iam.PolicyStatement({ sid: "ResolveTargets", actions: ["tag:GetResources"], resources: ["*"] }),
    );

    const template = new fis.CfnExperimentTemplate(this, "BlockRegion", {
      description: `DAT401: ${this.region}'s Aurora DSQL endpoint refuses connections`,
      roleArn: role.roleArn,
      stopConditions: [{ source: "none" }],
      targets: {
        cluster: { resourceType: "aws:dsql:cluster", resourceArns: [props.clusterArn], selectionMode: "ALL" },
      },
      actions: {
        refuseConnections: {
          actionId: "aws:dsql:cluster-connection-failure",
          description: "Refuse new connections and drop open ones",
          parameters: { duration: "PT10M", percentage: "100" },
          targets: { Clusters: "cluster" },
        },
      },
      tags: { Name: `dat401-block-${this.region}` },
    });
    template.node.addDependency(role);

    new cdk.CfnOutput(this, "TemplateId", { value: template.attrId });
  }
}
