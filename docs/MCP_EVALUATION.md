# Read-only agent evaluation fixture

Run `tests/integration.py --output <new-external-directory>` followed by `tests/agents.py --fixture <that-directory>` to retain this fixture. The session root is `<directory>/agents/sessions`; project ID is `mcp-demo`. Every question is independent and uses read-only tools after setup. Answers are stable for that fixture and are checked by the test driver through MCP. No model-driven task-success benchmark has been run; these questions prepare one without claiming competitive results.

```xml
<evaluation>
  <qa_pair>
    <question>Find the most recent restoration in mcp-demo history and inspect the state it restored. Which revision originally introduced the two-clip arrangement? Return its revision number.</question>
    <answer>2</answer>
  </qa_pair>
  <qa_pair>
    <question>Inspect the clip that comes second in the restored head and its historical source selection. At what source time does it end? Return whole seconds.</question>
    <answer>11</answer>
  </qa_pair>
  <qa_pair>
    <question>Find every apply action in history, inspect each resulting sequence and sum their durations. Return whole seconds.</question>
    <answer>20</answer>
  </qa_pair>
  <qa_pair>
    <question>Page through all revisions and inspect each snapshot. How many snapshots contain exactly two clips? Return the count.</question>
    <answer>2</answer>
  </qa_pair>
  <qa_pair>
    <question>Find the undo action in history and inspect its resulting state. How many clips remained after that undo?</question>
    <answer>1</answer>
  </qa_pair>
  <qa_pair>
    <question>Compare clip durations in the restored head and in its restoration target. Which clip is longest in both? Return its ID.</question>
    <answer>b</answer>
  </qa_pair>
  <qa_pair>
    <question>Using the restored head and sequence rate, calculate the first timeline frame of its second clip. Return a zero-based frame number.</question>
    <answer>100</answer>
  </qa_pair>
  <qa_pair>
    <question>Compare the head's source ranges with its asset duration. How many whole source seconds are outside the used interval?</question>
    <answer>2</answer>
  </qa_pair>
  <qa_pair>
    <question>Compare the restored head with its historical target, ignoring only revision numbers. Are all other snapshot fields identical? Return true or false.</question>
    <answer>true</answer>
  </qa_pair>
  <qa_pair>
    <question>Preview trimming clip b in the current head to source-in 5 seconds and duration 3 seconds. Verify the saved revision stays unchanged. What would the total sequence duration become? Return whole seconds.</question>
    <answer>7</answer>
  </qa_pair>
</evaluation>
```
