#!/usr/bin/env node
// Entry point: one site environment per synth, parameterized by context (see README.md).
import { App } from 'aws-cdk-lib';
import { readConfig } from '../lib/config';
import { buildSite } from '../lib/site';

const app = new App();
buildSite(app, readConfig(app.node));
app.synth();
