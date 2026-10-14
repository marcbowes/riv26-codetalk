// The blog API in one Region: Hono on Lambda (Node 24, arm64) behind a
// function URL, connecting to that Region's DSQL endpoint as blog_app.
import * as path from "node:path";
import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import * as lambda from "aws-cdk-lib/aws-lambda";
import * as nodejs from "aws-cdk-lib/aws-lambda-nodejs";
import * as logs from "aws-cdk-lib/aws-logs";
import type { Construct } from "constructs";
import { clusterIdFromArn } from "./config";
import { DB_USER, FUNCTION_NAME, apiRoleName } from "./names";

const DEMO_DIR = path.resolve(__dirname, "../..");

export interface ApiStackProps extends cdk.StackProps {
  /** This Region's cluster ARN. */
  clusterArn: string;
}

export class ApiStack extends cdk.Stack {
  readonly url: string;

  constructor(scope: Construct, id: string, props: ApiStackProps) {
    super(scope, id, props);

    const clusterEndpoint = `${clusterIdFromArn(props.clusterArn)}.dsql.${this.region}.on.aws`;

    // Deterministic name: setup.ts runs `AWS IAM GRANT blog_app TO '<this role's ARN>'`.
    const role = new iam.Role(this, "Role", {
      roleName: apiRoleName(this.region),
      description: "DAT401 blog API: connects to Aurora DSQL as blog_app",
      assumedBy: new iam.ServicePrincipal("lambda.amazonaws.com"),
      managedPolicies: [iam.ManagedPolicy.fromAwsManagedPolicyName("service-role/AWSLambdaBasicExecutionRole")],
    });
    role.addToPolicy(
      new iam.PolicyStatement({
        sid: "DsqlConnectAsAppRole",
        actions: ["dsql:DbConnect"],
        resources: [props.clusterArn],
      }),
    );

    const fn = new nodejs.NodejsFunction(this, "Function", {
      functionName: FUNCTION_NAME,
      description: "DAT401 multi-Region blog API (Hono + Drizzle on Aurora DSQL)",
      entry: path.join(DEMO_DIR, "api/src/lambda.ts"),
      handler: "handler",
      projectRoot: DEMO_DIR,
      depsLockFilePath: path.join(DEMO_DIR, "package-lock.json"),
      runtime: lambda.Runtime.NODEJS_24_X,
      architecture: lambda.Architecture.ARM_64,
      memorySize: 1024,
      timeout: cdk.Duration.seconds(10),
      role,
      environment: {
        CLUSTER_ENDPOINT: clusterEndpoint,
        DB_USER,
        NODE_OPTIONS: "--enable-source-maps",
      },
      logGroup: new logs.LogGroup(this, "Logs", {
        retention: logs.RetentionDays.ONE_WEEK,
        removalPolicy: cdk.RemovalPolicy.DESTROY,
      }),
      bundling: {
        // esbuild runs locally (no Docker). Bundle everything, including the AWS
        // SDK packages the DSQL connector uses for token signing, rather than
        // relying on the runtime's copy. pg-native is an optional pg add-on.
        format: nodejs.OutputFormat.CJS,
        target: "node24",
        minify: true,
        sourceMap: true,
        sourcesContent: false,
        externalModules: ["pg-native"],
        logLevel: nodejs.LogLevel.WARNING,
      },
    });

    const url = fn.addFunctionUrl({
      authType: lambda.FunctionUrlAuthType.NONE,
      // CORS lives here, not in the app: setting both duplicates the headers.
      cors: {
        allowedOrigins: ["*"],
        allowedMethods: [lambda.HttpMethod.GET, lambda.HttpMethod.POST],
        allowedHeaders: ["content-type"],
        exposedHeaders: ["x-region", "server-timing", "x-amzn-ErrorType", "retry-after"],
        maxAge: cdk.Duration.days(1),
      },
    });
    this.url = url.url;

    new cdk.CfnOutput(this, "ApiUrl", { value: url.url });
    new cdk.CfnOutput(this, "RoleArn", { value: role.roleArn });
    new cdk.CfnOutput(this, "FunctionName", { value: fn.functionName });
    new cdk.CfnOutput(this, "ClusterEndpoint", { value: clusterEndpoint });
  }
}
