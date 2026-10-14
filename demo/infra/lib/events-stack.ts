// CDC plumbing in REGION_A: Kinesis streams sized for DSQL CDC records and the
// service role DSQL assumes to write them.
//   dat401-cdc       live talk step 11 (CDC created in the console)
//   dat401-demo-cdc  the demo cluster's own CDC (deploy.sh, ENABLE_DEMO_CDC=1)
import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import * as kinesis from "aws-cdk-lib/aws-kinesis";
import type { Construct } from "constructs";
import { CDC_ROLE, CDC_STREAM, DEMO_CDC_STREAM } from "./names";

function cdcStream(scope: Construct, id: string, streamName: string): kinesis.Stream {
  const stream = new kinesis.Stream(scope, id, {
    streamName,
    // DSQL CDC requires ON_DEMAND and 10 MiB records.
    streamMode: kinesis.StreamMode.ON_DEMAND,
    encryption: kinesis.StreamEncryption.MANAGED,
    retentionPeriod: cdk.Duration.hours(24),
    removalPolicy: cdk.RemovalPolicy.DESTROY,
  });
  // The L2 doesn't expose MaxRecordSizeInKiB; set it on the L1.
  (stream.node.defaultChild as kinesis.CfnStream).maxRecordSizeInKiB = 10240;
  return stream;
}

export class EventsStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: cdk.StackProps) {
    super(scope, id, props);

    const cdc = cdcStream(this, "CdcStream", CDC_STREAM);
    const demoCdc = cdcStream(this, "DemoCdcStream", DEMO_CDC_STREAM);
    const streams = [cdc.streamArn, demoCdc.streamArn];

    const role = new iam.Role(this, "CdcRole", {
      roleName: CDC_ROLE,
      description: "Assumed by Aurora DSQL to write CDC records to the DAT401 Kinesis streams",
      assumedBy: new iam.ServicePrincipal("dsql.amazonaws.com", {
        conditions: {
          StringEquals: { "aws:SourceAccount": this.account },
          // Any cluster's streams in this Region and account: the live talk's
          // cluster doesn't exist yet when this deploys.
          ArnLike: { "aws:SourceArn": `arn:${this.partition}:dsql:${this.region}:${this.account}:cluster/*/stream/*` },
        },
      }),
    });
    role.addToPolicy(
      new iam.PolicyStatement({
        sid: "KinesisAccess",
        actions: ["kinesis:PutRecord", "kinesis:PutRecords", "kinesis:DescribeStreamSummary", "kinesis:ListShards"],
        resources: streams,
      }),
    );
    // Only needed for customer managed keys; harmless otherwise (per the CDC docs).
    role.addToPolicy(
      new iam.PolicyStatement({
        sid: "KMSAccess",
        actions: ["kms:GenerateDataKey"],
        resources: ["arn:aws:kms:*:*:key/*"],
        conditions: {
          StringEquals: {
            "kms:ViaService": `kinesis.${this.region}.amazonaws.com`,
            "kms:EncryptionContext:aws:kinesis:arn": streams,
            "aws:ResourceAccount": "${aws:PrincipalAccount}",
          },
        },
      }),
    );

    new cdk.CfnOutput(this, "CdcStreamName", { value: cdc.streamName });
    new cdk.CfnOutput(this, "CdcStreamArn", { value: cdc.streamArn });
    new cdk.CfnOutput(this, "DemoCdcStreamName", { value: demoCdc.streamName });
    new cdk.CfnOutput(this, "DemoCdcStreamArn", { value: demoCdc.streamArn });
    new cdk.CfnOutput(this, "CdcRoleArn", { value: role.roleArn });
  }
}
