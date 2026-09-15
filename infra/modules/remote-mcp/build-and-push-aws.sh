#!/usr/bin/env bash
# Build and push the mind-palace-mcp-remote Docker image to ECR.
#
# Run this BEFORE `terraform apply` (and whenever the code changes). It is
# intentionally separate from Terraform: image builds belong to the code
# lifecycle, infra to Terraform.
#
# Usage:
#   ./build-and-push.sh <aws-account-id> <region> [repo-name] [tag] [aws-profile]
#
# Example:
#   ./build-and-push.sh 840854325365 us-east-1 mind-palace-mcp-remote latest command-center
#
# Must be run from the repo root (where crates/ lives) OR set REPO_ROOT.

set -euo pipefail

ACCOUNT_ID="${1:?account id required}"
REGION="${2:?region required}"
REPO_NAME="${3:-mind-palace-mcp-remote}"
TAG="${4:-latest}"
PROFILE="${5:-}"

PROFILE_FLAG=""
if [ -n "$PROFILE" ]; then
  PROFILE_FLAG="--profile $PROFILE"
  export AWS_PROFILE="$PROFILE"
fi

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "$0")/../../.." && pwd)}"
ECR_HOST="${ACCOUNT_ID}.dkr.ecr.${REGION}.amazonaws.com"
IMAGE_URI="${ECR_HOST}/${REPO_NAME}:${TAG}"

echo "Repo root:  $REPO_ROOT"
echo "Image URI:  $IMAGE_URI"

# 1. Ensure the ECR repo exists (idempotent).
if ! aws ecr describe-repositories --repository-names "$REPO_NAME" \
      --region "$REGION" $PROFILE_FLAG >/dev/null 2>&1; then
  echo "Creating ECR repository $REPO_NAME ..."
  aws ecr create-repository --repository-name "$REPO_NAME" \
    --image-scanning-configuration scanOnPush=true \
    --region "$REGION" $PROFILE_FLAG >/dev/null
fi

# 2. Log in to ECR.
aws ecr get-login-password --region "$REGION" $PROFILE_FLAG \
  | docker login --username AWS --password-stdin "$ECR_HOST"

# 3. Build (use --network host to avoid DNS issues in some environments).
docker build --network host \
  -f "$REPO_ROOT/crates/mind-palace-mcp/Dockerfile.remote" \
  -t "$REPO_NAME:$TAG" \
  "$REPO_ROOT"

# 4. Tag and push.
docker tag "$REPO_NAME:$TAG" "$IMAGE_URI"
docker push "$IMAGE_URI"

echo ""
echo "Pushed: $IMAGE_URI"
echo "Use this as the module 'image' input."
