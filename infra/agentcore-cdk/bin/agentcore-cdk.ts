#!/usr/bin/env node
import * as cdk from 'aws-cdk-lib';
import { AgentcoreStack } from '../lib/agentcore-stack';
import { AgentcoreBootstrapStack } from '../lib/bootstrap-stack';

const app = new cdk.App();

const env = {
  account: process.env.CDK_DEFAULT_ACCOUNT,
  region: process.env.CDK_DEFAULT_REGION,
};

const githubRepo = app.node.tryGetContext('githubRepo') ?? 'nigeldunn/agents';
const githubRef = app.node.tryGetContext('githubRef');

new AgentcoreBootstrapStack(app, 'AgentcoreBootstrapStack', {
  env,
  githubRepo,
  githubRef,
});

const vpcId = app.node.tryGetContext('vpcId');
const orchestratorSgId = app.node.tryGetContext('orchestratorSgId');
const clusterName = app.node.tryGetContext('clusterName');
const createVpcEndpoints = app.node.tryGetContext('createVpcEndpoints') === 'true'
  || app.node.tryGetContext('createVpcEndpoints') === true;
const imageTag = app.node.tryGetContext('imageTag');

if (vpcId && orchestratorSgId && imageTag) {
  new AgentcoreStack(app, 'AgentcoreStack', {
    env,
    vpcId,
    orchestratorSgId,
    clusterName,
    createVpcEndpoints,
    imageTag,
  });
}
