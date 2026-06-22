---
description: Snapshot AWS infra health — EC2, S3, ECR, CloudWatch logs
allowed-tools: Bash, Read
---

# AWS infrastructure status

Run read-only AWS checks. **No mutations.** Report a structured summary.

## Identity check

```bash
aws sts get-caller-identity
```

Confirm the account matches what's expected for suwappu-db (look in `terraform/` or `scripts/bootstrap.sh` for the canonical account ID — don't hardcode it here).

## Compute

```bash
aws ec2 describe-instances \
  --filters "Name=tag:Project,Values=suwappu-db" \
  --query 'Reservations[].Instances[].{Id:InstanceId,State:State.Name,Type:InstanceType,Launch:LaunchTime,Name:Tags[?Key==`Name`]|[0].Value}' \
  --output table
```

Flag any instance that is `stopped`, `stopping`, or `terminated`.

## Storage

```bash
aws s3 ls | grep -i suwappu || echo "no suwappu-* buckets"
```

## Container registry

```bash
aws ecr describe-repositories \
  --query 'repositories[?contains(repositoryName, `suwappu`)].{Name:repositoryName,URI:repositoryUri}' \
  --output table 2>/dev/null || echo "no suwappu ECR repos"
```

## Recent logs (validator shadow)

```bash
# Find log groups, then tail the most recent
aws logs describe-log-groups \
  --log-group-name-prefix /suwappu-db \
  --query 'logGroups[].logGroupName' --output text 2>/dev/null
```

If a `/suwappu-db/validator-shadow` group exists, tail the last 5 minutes:

```bash
aws logs tail /suwappu-db/validator-shadow --since 5m --format short 2>/dev/null | tail -50
```

## Report format

```
Identity      ✓ <account-id>
EC2           <count> running, <count> stopped — flag any stopped
S3            <bucket count> + names
ECR           <repo count> + names
Logs          <log-group count> + last-event timestamps
Drift         any unexpected resources, missing expected ones, or stopped instances
```
