// "Look at it scale": one CloudWatch dashboard (in REGION_A) with cross-Region
// widgets for both Lambda functions and both DSQL clusters.
//
// Metric names: Lambda from AWS/Lambda; DSQL from the AWS/AuroraDSQL namespace,
// dimension ClusterId (TotalTransactions, ReadOnlyTransactions, OccConflicts,
// CommitLatency, TotalDPU, ...), checked against a deployed cluster; new
// connections are AWS/Usage CallCount for the DbConnect API.
import * as cdk from "aws-cdk-lib";
import * as cw from "aws-cdk-lib/aws-cloudwatch";
import type { Construct } from "constructs";
import { DASHBOARD_NAME, FUNCTION_NAME, REGION_NAMES } from "./names";

export interface DashboardStackProps extends cdk.StackProps {
  /** [REGION_A, REGION_B] with each Region's cluster id. */
  clusters: { region: string; clusterId: string }[];
  witnessRegion: string;
}

// Categorical slots 1 and 2 (validated as a pair): color follows the Region on
// every widget.
const REGION_COLORS = ["#2a78d6", "#eb6834"] as const;

const MINUTE = cdk.Duration.minutes(1);

export class DashboardStack extends cdk.Stack {
  readonly url: string;

  constructor(scope: Construct, id: string, props: DashboardStackProps) {
    super(scope, id, props);

    const perRegion = (
      make: (c: { region: string; clusterId: string }, color: string) => cw.IMetric,
    ): cw.IMetric[] => props.clusters.map((c, i) => make(c, REGION_COLORS[i % 2] as string));

    const lambda = (metricName: string, statistic: string, labelSuffix = "") =>
      perRegion(
        (c, color) =>
          new cw.Metric({
            namespace: "AWS/Lambda",
            metricName,
            dimensionsMap: { FunctionName: FUNCTION_NAME },
            statistic,
            period: MINUTE,
            region: c.region,
            label: `${c.region}${labelSuffix}`,
            color,
          }),
      );

    const dsql = (metricName: string, statistic: string) =>
      perRegion(
        (c, color) =>
          new cw.Metric({
            namespace: "AWS/AuroraDSQL",
            metricName,
            dimensionsMap: { ClusterId: c.clusterId },
            statistic,
            period: MINUTE,
            region: c.region,
            label: `${c.region} (${c.clusterId})`,
            color,
          }),
      );

    // New connections: AWS/Usage CallCount for the DbConnect API (one per IAM-authenticated
    // connection). Verified against a deployed cluster; there's no open-connection gauge.
    const connections = perRegion(
      (c, color) =>
        new cw.Metric({
          namespace: "AWS/Usage",
          metricName: "CallCount",
          dimensionsMap: {
            Service: "AuroraDSQL",
            Type: "API",
            Resource: "DbConnect",
            Class: "None",
            ResourceId: `cluster/${c.clusterId}`,
          },
          statistic: "Sum",
          period: MINUTE,
          region: c.region,
          label: c.region,
          color,
        }),
    );

    const graph = (title: string, left: cw.IMetric[], width: number, extra: Partial<cw.GraphWidgetProps> = {}) =>
      new cw.GraphWidget({
        title,
        left,
        width,
        height: 6,
        period: MINUTE,
        liveData: true,
        legendPosition: cw.LegendPosition.BOTTOM,
        leftYAxis: { min: 0, showUnits: false },
        ...extra,
      });

    const tile = (title: string, metrics: cw.IMetric[]) =>
      new cw.SingleValueWidget({ title, metrics, width: 8, height: 4, sparkline: true, period: MINUTE });

    const regionsText = props.clusters
      .map((c, i) => `**${c.region}** (${REGION_NAMES[c.region] ?? c.region}, ${i === 0 ? "blue" : "orange"})`)
      .join(" and ");

    const dashboard = new cw.Dashboard(this, "Dashboard", {
      dashboardName: DASHBOARD_NAME,
      defaultInterval: cdk.Duration.minutes(30),
      widgets: [
        [
          new cw.TextWidget({
            markdown:
              `# DAT401 blog: one database, two Regions\n` +
              `Lambda + Aurora DSQL in ${regionsText}; witness ${props.witnessRegion}. ` +
              `Counts are per minute. No provisioning: watch Lambda environments and DSQL work scale with the load.`,
            width: 24,
            height: 2,
          }),
        ],
        [
          tile("Requests per minute", lambda("Invocations", "Sum")),
          tile("DSQL transactions per minute", dsql("TotalTransactions", "Sum")),
          tile("Lambda environments in use", lambda("ConcurrentExecutions", "Maximum")),
        ],
        [
          graph("Requests per minute", lambda("Invocations", "Sum"), 12),
          graph("Concurrent Lambda environments", lambda("ConcurrentExecutions", "Maximum"), 12),
        ],
        [
          graph("Lambda duration p50 (ms)", lambda("Duration", "p50"), 6),
          graph("Lambda duration p99 (ms)", lambda("Duration", "p99"), 6),
          graph("Lambda errors", lambda("Errors", "Sum"), 6),
          graph("Lambda throttles", lambda("Throttles", "Sum"), 6),
        ],
        [
          graph("DSQL transactions per minute", dsql("TotalTransactions", "Sum"), 8),
          graph("DSQL read-only transactions per minute", dsql("ReadOnlyTransactions", "Sum"), 8),
          graph("DSQL OCC conflicts", dsql("OccConflicts", "Sum"), 8),
        ],
        [
          graph("DSQL DPUs per minute", dsql("TotalDPU", "Sum"), 8),
          graph("DSQL commit latency (ms)", dsql("CommitLatency", "Average"), 8),
          graph("DSQL new connections per minute", connections, 8),
        ],
      ],
    });

    this.url = `https://${this.region}.console.aws.amazon.com/cloudwatch/home?region=${this.region}#dashboards/dashboard/${dashboard.dashboardName}`;
    new cdk.CfnOutput(this, "DashboardName", { value: DASHBOARD_NAME });
    new cdk.CfnOutput(this, "DashboardUrl", { value: this.url });
  }
}
