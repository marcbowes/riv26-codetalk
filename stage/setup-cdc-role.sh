#!/usr/bin/env bash
# The IAM role DSQL assumes to write CDC records to Kinesis (step 10).
# Set up once, before the talk; safe to re-run. The console makes an
# equivalent role per cluster; this one serves every cluster in the account
# and Region (each rehearsal makes a new cluster), and can only write to
# the dat401-cdc streams.
#
#   AWS_PROFILE=bowes AWS_REGION=us-west-2 stage/setup-cdc-role.sh
set -euo pipefail

ROLE=dat401-cdc
REGION="${AWS_REGION:-us-west-2}"
ACCOUNT="$(aws sts get-caller-identity --query Account --output text)"

trust=$(cat <<JSON
{
  "Version": "2012-10-17",
  "Statement": [{
    "Sid": "DsqlStreams",
    "Effect": "Allow",
    "Principal": {"Service": "dsql.amazonaws.com"},
    "Action": "sts:AssumeRole",
    "Condition": {
      "StringEquals": {"aws:SourceAccount": "$ACCOUNT"},
      "ArnLike": {"aws:SourceArn": "arn:aws:dsql:$REGION:$ACCOUNT:cluster/*/stream/*"}
    }
  }]
}
JSON
)

permissions=$(cat <<JSON
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "KinesisAccess",
      "Effect": "Allow",
      "Action": ["kinesis:PutRecord", "kinesis:PutRecords", "kinesis:DescribeStreamSummary", "kinesis:ListShards"],
      "Resource": "arn:aws:kinesis:$REGION:$ACCOUNT:stream/dat401-cdc*"
    },
    {
      "Sid": "KMSAccess",
      "Effect": "Allow",
      "Action": "kms:GenerateDataKey",
      "Resource": "arn:aws:kms:$REGION:$ACCOUNT:key/*",
      "Condition": {
        "StringEquals": {"kms:ViaService": "kinesis.$REGION.amazonaws.com"},
        "StringLike": {"kms:EncryptionContext:aws:kinesis:arn": "arn:aws:kinesis:$REGION:$ACCOUNT:stream/dat401-cdc*"}
      }
    }
  ]
}
JSON
)

if aws iam get-role --role-name "$ROLE" >/dev/null 2>&1; then
  aws iam update-assume-role-policy --role-name "$ROLE" --policy-document "$trust"
  echo "Updated $ROLE's trust policy."
else
  aws iam create-role --role-name "$ROLE" --assume-role-policy-document "$trust" \
    --description "DAT401: lets Aurora DSQL CDC streams write to the dat401-cdc Kinesis streams" \
    --tags Key=project,Value=dat401-reinvent-2026 >/dev/null
  echo "Created $ROLE."
fi
aws iam put-role-policy --role-name "$ROLE" --policy-name kinesis-write --policy-document "$permissions"
aws iam get-role --role-name "$ROLE" --query Role.Arn --output text
