// The load generator host for the finale: one Graviton instance in REGION_A
// (close to the audience's Region), reached through SSM Session Manager. No
// inbound ports, no SSH keys. Setup installs Rust, clones the repo and builds
// loadgen and events (see userData); `loadgen --run-on-ec2` does the rest.
import * as cdk from "aws-cdk-lib";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as iam from "aws-cdk-lib/aws-iam";
import type { Construct } from "constructs";

export interface LoadgenStackProps extends cdk.StackProps {
  /** Public Git URL to clone. */
  repoUrl: string;
  instanceType: string;
  /** AZ for the host. EC2 capacity per type varies by AZ, so deploy.sh picks one that works. */
  availabilityZone?: string;
}

/** Root script: idempotent, so it can simply run again after a reboot. */
function setupScript(repoUrl: string): string {
  return `#!/bin/bash
set -euxo pipefail
exec >>/var/log/dat401-setup.log 2>&1
echo "=== $(date -Is) setup run, boot $(cat /proc/sys/kernel/random/boot_id)"

# C toolchain + cmake for aws-lc-rs (rustls), git, tmux. dnf may be busy patching.
for attempt in $(seq 1 20); do
  dnf install -y gcc gcc-c++ make cmake clang git tmux && break
  echo "dnf failed (attempt $attempt; patching holds the lock?), retrying in 30 s"
  sleep 30
done
command -v cmake && command -v git

# One socket per in-flight request: lots of file descriptors and ephemeral ports.
printf '%s\\n' '* soft nofile 1048576' '* hard nofile 1048576' > /etc/security/limits.d/90-loadgen.conf
printf '%s\\n' 'net.ipv4.ip_local_port_range = 1024 65535' 'net.ipv4.tcp_tw_reuse = 1' \\
  'net.core.somaxconn = 4096' > /etc/sysctl.d/90-loadgen.conf
sysctl --system

sudo -u ec2-user -H bash -lc '
  set -euxo pipefail
  [ -x "$HOME/.cargo/bin/cargo" ] || curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  source "$HOME/.cargo/env"
  if [ -d "$HOME/riv26-codetalk/.git" ]; then
    git -C "$HOME/riv26-codetalk" pull --ff-only
  else
    rm -rf "$HOME/riv26-codetalk"
    git clone ${repoUrl} "$HOME/riv26-codetalk"
  fi
  cargo build --release --manifest-path "$HOME/riv26-codetalk/demo/loadgen/Cargo.toml"
  cargo build --release --manifest-path "$HOME/riv26-codetalk/events/Cargo.toml"
  touch "$HOME/READY"
'
echo "=== $(date -Is) setup done"
`;
}

const SETUP_UNIT = `[Unit]
Description=DAT401 load generator setup (re-runs at boot until done)
Wants=network-online.target
After=network-online.target
ConditionPathExists=!/home/ec2-user/READY

[Service]
Type=oneshot
ExecStart=/usr/local/bin/dat401-setup.sh
Restart=on-failure
RestartSec=30
TimeoutStartSec=infinity

[Install]
WantedBy=multi-user.target
`;

/**
 * Setup runs as a systemd one-shot service rather than straight from user data:
 * the account's SSM patch association (AWS-RunPatchBaseline) patches and
 * reboots new instances within minutes, which killed a plain user-data run
 * halfway. The service re-runs at every boot until ~ec2-user/READY exists.
 * Logs: /var/log/dat401-setup.log.
 */
function userData(repoUrl: string): string {
  return `#!/bin/bash
set -euxo pipefail
cat > /usr/local/bin/dat401-setup.sh <<'DAT401_SETUP_END'
${setupScript(repoUrl)}DAT401_SETUP_END
chmod 0755 /usr/local/bin/dat401-setup.sh
cat > /etc/systemd/system/dat401-setup.service <<'DAT401_UNIT_END'
${SETUP_UNIT}DAT401_UNIT_END
systemctl daemon-reload
systemctl enable dat401-setup.service
systemctl start --no-block dat401-setup.service
`;
}

export class LoadgenStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: LoadgenStackProps) {
    super(scope, id, props);

    // Its own tiny VPC: a public subnet per AZ (so the host can move to an AZ
    // with capacity), no NAT. Outbound only (GitHub, crates.io, CloudFront);
    // nothing can connect in.
    const vpc = new ec2.Vpc(this, "Vpc", {
      maxAzs: 4,
      natGateways: 0,
      subnetConfiguration: [{ name: "public", subnetType: ec2.SubnetType.PUBLIC, cidrMask: 24 }],
    });
    const sg = new ec2.SecurityGroup(this, "Sg", {
      vpc,
      description: "DAT401 load generator: outbound only (access is via SSM)",
      allowAllOutbound: true,
    });

    const role = new iam.Role(this, "Role", {
      assumedBy: new iam.ServicePrincipal("ec2.amazonaws.com"),
      managedPolicies: [iam.ManagedPolicy.fromAwsManagedPolicyName("AmazonSSMManagedInstanceCore")],
    });

    const instance = new ec2.Instance(this, "Host", {
      vpc,
      vpcSubnets: {
        subnetType: ec2.SubnetType.PUBLIC,
        ...(props.availabilityZone ? { availabilityZones: [props.availabilityZone] } : {}),
      },
      securityGroup: sg,
      role,
      instanceType: new ec2.InstanceType(props.instanceType),
      machineImage: ec2.MachineImage.latestAmazonLinux2023({ cpuType: ec2.AmazonLinuxCpuType.ARM_64 }),
      requireImdsv2: true,
      blockDevices: [
        {
          deviceName: "/dev/xvda",
          volume: ec2.BlockDeviceVolume.ebs(40, { volumeType: ec2.EbsDeviceVolumeType.GP3, encrypted: true }),
        },
      ],
      userData: ec2.UserData.custom(userData(props.repoUrl)),
      // A changed setup script means a fresh host (user data only runs on first boot).
      userDataCausesReplacement: true,
    });
    cdk.Tags.of(instance).add("Name", "dat401-loadgen");

    new cdk.CfnOutput(this, "InstanceId", { value: instance.instanceId });
    new cdk.CfnOutput(this, "InstanceType", { value: props.instanceType });
    new cdk.CfnOutput(this, "AvailabilityZone", { value: instance.instanceAvailabilityZone });
  }
}
