// One CloudFront distribution for everything:
//   /*                 the React app, from a private S3 bucket (OAC)
//   /api/<region>/*    that Region's API, a Lambda function URL with AWS_IAM auth (OAC)
//   /config.json       generated: the Regions and their /api/<region>/ paths
// Same origin for page and APIs, so no CORS. The APIs aren't public: only this
// distribution may call them (deploy.sh grants it on the API stacks).
//
// Lambda OAC can't hash request bodies, so POST/PUT clients must send
// x-amz-content-sha256 (the web app and loadgen do).
import * as fs from "node:fs";
import * as path from "node:path";
import * as cdk from "aws-cdk-lib";
import * as cloudfront from "aws-cdk-lib/aws-cloudfront";
import * as origins from "aws-cdk-lib/aws-cloudfront-origins";
import * as s3 from "aws-cdk-lib/aws-s3";
import * as s3deploy from "aws-cdk-lib/aws-s3-deployment";
import type { Construct } from "constructs";
import { REGION_NAMES } from "./names";

const WEB_DIST = path.resolve(__dirname, "../../web/dist");

export interface WebStackProps extends cdk.StackProps {
  regions: { region: string; functionUrl: string }[];
  witnessRegion: string;
}

/** Path prefix CloudFront routes to a Region's API (lambda.ts strips it). */
export const apiPath = (region: string) => `/api/${region}/`;

export class WebStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: WebStackProps) {
    super(scope, id, props);

    if (!fs.existsSync(path.join(WEB_DIST, "index.html"))) {
      throw new Error(`${WEB_DIST}/index.html not found: run \`npm run build -w @dat401/web\` first`);
    }

    const bucket = new s3.Bucket(this, "Site", {
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      encryption: s3.BucketEncryption.S3_MANAGED,
      enforceSSL: true,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
      autoDeleteObjects: true,
    });

    const origin = origins.S3BucketOrigin.withOriginAccessControl(bucket);

    const lambdaOac = new cloudfront.FunctionUrlOriginAccessControl(this, "ApiOac", {
      description: "DAT401: sign requests to the blog API function URLs",
      signing: cloudfront.Signing.SIGV4_ALWAYS,
    });
    const apiBehaviors: Record<string, cloudfront.BehaviorOptions> = {};
    for (const r of props.regions) {
      apiBehaviors[`${apiPath(r.region)}*`] = {
        origin: new origins.HttpOrigin(new URL(r.functionUrl).host, {
          originId: `api-${r.region}`,
          protocolPolicy: cloudfront.OriginProtocolPolicy.HTTPS_ONLY,
          originAccessControlId: lambdaOac.originAccessControlId,
          readTimeout: cdk.Duration.seconds(30),
        }),
        viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.HTTPS_ONLY,
        allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
        cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
        // Everything but Host: SigV4 is computed for the function URL's own host.
        originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
      };
    }

    const distribution = new cloudfront.Distribution(this, "Cdn", {
      comment: "DAT401 multi-Region blog",
      defaultRootObject: "index.html",
      priceClass: cloudfront.PriceClass.PRICE_CLASS_100,
      httpVersion: cloudfront.HttpVersion.HTTP2_AND_3,
      defaultBehavior: {
        origin,
        viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        cachePolicy: cloudfront.CachePolicy.CACHING_OPTIMIZED,
        compress: true,
      },
      additionalBehaviors: {
        // Runtime config must never be stale.
        "/config.json": {
          origin,
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
        },
        ...apiBehaviors,
      },
    });

    new s3deploy.BucketDeployment(this, "Deploy", {
      sources: [
        s3deploy.Source.asset(WEB_DIST),
        s3deploy.Source.jsonData("config.json", {
          witnessRegion: props.witnessRegion,
          regions: props.regions.map((r) => ({
            region: r.region,
            apiUrl: apiPath(r.region),
            name: REGION_NAMES[r.region] ?? r.region,
          })),
        }),
      ],
      destinationBucket: bucket,
      distribution,
      distributionPaths: ["/*"], // invalidate on every deploy
      memoryLimit: 512,
    });

    new cdk.CfnOutput(this, "WebUrl", { value: `https://${distribution.distributionDomainName}` });
    new cdk.CfnOutput(this, "DistributionId", { value: distribution.distributionId });
    new cdk.CfnOutput(this, "DistributionArn", { value: distribution.distributionArn });
    new cdk.CfnOutput(this, "BucketName", { value: bucket.bucketName });
  }
}
