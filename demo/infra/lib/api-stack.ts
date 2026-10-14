// The blog API in one Region: Hono on Lambda (Node 24, arm64) behind an
// IAM-authenticated function URL, connecting to that Region's DSQL endpoint as
// blog_app. Only the Dat401-Web CloudFront distribution may call the URL (OAC):
// public function URLs get their resource policy stripped in this account.
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
  /** The Dat401-Web distribution, once it exists (deploy.sh passes it on the second pass). */
  distributionArn?: string;
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

    // No CORS: the browser calls the API through CloudFront on the page's own origin.
    const url = fn.addFunctionUrl({ authType: lambda.FunctionUrlAuthType.AWS_IAM });
    this.url = url.url;

    // Function URLs need both permissions (since October 2025), scoped to the distribution.
    if (props.distributionArn) {
      const cloudfrontPrincipal = new iam.ServicePrincipal("cloudfront.amazonaws.com");
      fn.addPermission("CloudFrontInvokeFunctionUrl", {
        principal: cloudfrontPrincipal,
        action: "lambda:InvokeFunctionUrl",
        functionUrlAuthType: lambda.FunctionUrlAuthType.AWS_IAM,
        sourceArn: props.distributionArn,
      });
      fn.addPermission("CloudFrontInvokeFunction", {
        principal: cloudfrontPrincipal,
        action: "lambda:InvokeFunction",
        invokedViaFunctionUrl: true,
        sourceArn: props.distributionArn,
      });
    }

    new cdk.CfnOutput(this, "FunctionUrl", { value: url.url });
    new cdk.CfnOutput(this, "RoleArn", { value: role.roleArn });
    new cdk.CfnOutput(this, "FunctionName", { value: fn.functionName });
    new cdk.CfnOutput(this, "ClusterEndpoint", { value: clusterEndpoint });
  }
}
