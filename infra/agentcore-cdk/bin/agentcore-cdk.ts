#!/usr/bin/env node
import * as cdk from 'aws-cdk-lib';
import { AgentcoreStack } from '../lib/agentcore-stack';

const app = new cdk.App();

const vpcId = app.node.tryGetContext('vpcId');
const orchestratorSgId = app.node.tryGetContext('orchestratorSgId');
const clusterName = app.node.tryGetContext('clusterName');
const createVpcEndpoints = app.node.tryGetContext('createVpcEndpoints') === 'true'
  || app.node.tryGetContext('createVpcEndpoints') === true;

if (!vpcId) {
  throw new Error('Missing required context: -c vpcId=vpc-xxxx');
}
if (!orchestratorSgId) {
  throw new Error('Missing required context: -c orchestratorSgId=sg-xxxx');
}

new AgentcoreStack(app, 'AgentcoreStack', {
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: process.env.CDK_DEFAULT_REGION,
  },
  vpcId,
  orchestratorSgId,
  clusterName,
  createVpcEndpoints,
});
