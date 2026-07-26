terraform {
  required_version = ">= 1.0"

  # Configure your backend:
  # backend "s3" {
  #   bucket         = "your-terraform-state"
  #   key            = "mind-palace"
  #   region         = "us-west-2"
  #   encrypt        = true
  #   dynamodb_table = "terraform-state-lock"
  # }

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = var.region

  default_tags {
    tags = {
      Project   = "mind-palace"
      ManagedBy = "terraform"
    }
  }
}
