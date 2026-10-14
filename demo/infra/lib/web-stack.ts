// The static React app: private S3 bucket behind CloudFront (OAC), plus a
// generated /config.json telling the app which Regional APIs to call.
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
  regions: { region: string; apiUrl: string }[];
  witnessRegion: string;
}

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
      },
    });

    new s3deploy.BucketDeployment(this, "Deploy", {
      sources: [
        s3deploy.Source.asset(WEB_DIST),
        s3deploy.Source.jsonData("config.json", {
          witnessRegion: props.witnessRegion,
          regions: props.regions.map((r) => ({ ...r, name: REGION_NAMES[r.region] ?? r.region })),
        }),
      ],
      destinationBucket: bucket,
      distribution,
      distributionPaths: ["/*"], // invalidate on every deploy
      memoryLimit: 512,
    });

    new cdk.CfnOutput(this, "WebUrl", { value: `https://${distribution.distributionDomainName}` });
    new cdk.CfnOutput(this, "DistributionId", { value: distribution.distributionId });
    new cdk.CfnOutput(this, "BucketName", { value: bucket.bucketName });
  }
}
