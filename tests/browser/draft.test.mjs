import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createInterface } from 'node:readline';
import test from 'node:test';
import { chromium } from 'playwright-core';

test('并发编辑被拒绝后保留本页草稿，重新打开对象可读到已提交正文', async (t) => {
  const root = await mkdtemp(path.join(tmpdir(), 'braid-console-draft-'));
  const binary = path.resolve(process.env.BRAID_BINARY || 'target/debug/braid');
  const config = path.join(root, 'platform.toml');
  const checkout = path.join(root, 'source');
  let server, exited, browser;
  t.after(async () => {
    try {
      await browser?.close();
    } finally {
      server?.kill('SIGKILL');
      await exited;
      await rm(root, { recursive: true, force: true });
    }
  });
  execFileSync('git', ['init', '-q', '-b', 'main', checkout]);
  execFileSync('git', ['-C', checkout, '-c', 'user.name=Braid', '-c', 'user.email=test@example.invalid',
    '-c', 'commit.gpgsign=false', 'commit', '-q', '--allow-empty', '-m', 'fixture']);
  execFileSync(binary, ['platform', 'init', '--name', 'fixture', '--listen', '127.0.0.1:0',
    '--source', checkout, '--config', config]);

  server = spawn(binary, ['console', '--platform-config', config, '--listen', '127.0.0.1:0',
    '--database', path.join(root, 'console.sqlite3')], { stdio: ['ignore', 'pipe', 'inherit'] });
  exited = once(server, 'exit');
  const lines = createInterface({ input: server.stdout });
  const [line] = await Promise.race([
    once(lines, 'line', { signal: AbortSignal.timeout(15_000) }),
    exited.then(([code]) => { throw new Error(`Console 在报告监听地址前退出，退出码 ${code}`); }),
  ]);
  const endpoint = line.match(/^Braid Console (http:\/\/127\.0\.0\.1:\d+)$/)?.[1];
  assert.ok(endpoint, line);
  browser = await chromium.launch({ executablePath: process.env.CHROME_BIN });
  const context = await browser.newContext();
  context.setDefaultTimeout(10_000);
  const first = await context.newPage();
  const second = await context.newPage();
  const url = endpoint + '/#objects';
  await first.goto(url);
  await first.getByLabel('标题', { exact: true }).fill('并发编辑');
  await first.getByLabel('正文', { exact: true }).fill('原始正文');
  await first.getByRole('button', { name: '创建', exact: true }).click();
  const item = { name: '#1 并发编辑', exact: true };
  await first.getByRole('button', item).click();
  await second.goto(url);
  await second.getByRole('button', item).click();
  const editor = (page) => page.locator('section').filter({
    has: page.getByRole('heading', { name: '编辑与控制', exact: true }),
  }).getByLabel('正文', { exact: true });
  const save = { name: '保存标题与正文', exact: true };
  await editor(second).fill('第二位操作者的草稿');
  await editor(first).fill('第一位操作者已保存');
  await first.getByRole('button', save).click();
  await first.getByText('第一位操作者已保存', { exact: true }).filter({ visible: true }).first().waitFor();
  await second.getByRole('button', save).click();
  await second.getByRole('status').getByText(/object #1 revision is \d+, expected \d+/).waitFor();
  assert.equal(await editor(second).inputValue(), '第二位操作者的草稿');

  await second.getByRole('button', { name: '刷新', exact: true }).click();
  const confirm = second.getByRole('alertdialog');
  await confirm.getByRole('button', { name: '取消', exact: true }).click();
  assert.equal(await editor(second).inputValue(), '第二位操作者的草稿');
  await second.getByRole('button', { name: '刷新', exact: true }).click();
  await confirm.getByRole('button', { name: '继续', exact: true }).click();
  await second.getByRole('button', item).click();
  assert.equal(await editor(second).inputValue(), '第一位操作者已保存');
});
