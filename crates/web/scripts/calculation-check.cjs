const { chromium } = require("playwright-core");
const assert = require("node:assert/strict");
const { signInFreshAdmin } = require("./sign-in.cjs");
(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || "/usr/bin/chromium", headless: true, args: ["--no-sandbox", "--disable-dev-shm-usage"] });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1e3 } });
  const base = process.env.VULSE_TEST_URL || "http://127.0.0.1:8098";
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  try {
    const { run } = await signInFreshAdmin(page, base, "calc");
    const blueprint = { fields: [{ name: "price", type: "number" }, { name: "quantity", type: "number" }, { name: "first_name", type: "string" }, { name: "last_name", type: "string" }, { name: "scores", type: "array", items: "number" }, { name: "double", type: "calculated", formula: "=[price]*2" }, { name: "total", type: "calculated", formula: "=ROUND([price]*[quantity],2)" }] };
    const created = await page.request.post(base + "/api/v1/collections", { data: { name: "calculation-tests-" + run, visibility: "private", blueprint } });
    assert.equal(created.status(), 201);
    const collection = await created.json();
    const id = collection.id;
    const made = await page.request.post(base + `/api/v1/collections/${id}/documents`, { data: { body: { price: 24.5, quantity: 3, first_name: "Ada", last_name: "Lovelace", scores: [1, 2, 3] } } });
    assert.equal(made.status(), 201);
    const record = await made.json();
    await page.goto(base + `/collections/${id}`);
    await page.getByRole("button", { name: "Fields", exact: true }).click();
    const double = page.getByRole("region", { name: "double calculation", exact: true });
    const total = page.getByRole("region", { name: "total calculation", exact: true });
    await double.getByLabel("Calculation", { exact: true }).waitFor();
    assert.equal(await double.getByLabel("Calculation", { exact: true }).inputValue(), "multiply");
    assert.equal(await double.getByLabel("Multiply by 2 fixed value").inputValue(), "2");
    assert.equal(await total.getByLabel("Calculation", { exact: true }).first().inputValue(), "ROUND");
    const product = total.getByRole("group", { name: "Value to round 1 nested calculation", exact: true });
    assert.equal(await product.getByLabel("Calculation", { exact: true }).inputValue(), "multiply");
    await total.getByLabel("Preview values").selectOption(record.id);
    assert.equal(await total.getByLabel("Calculation preview result").innerText(), "73.5");
    await total.getByRole("button", { name: "Edit formula", exact: true }).click();
    assert.equal(await total.getByLabel("Formula", { exact: true }).inputValue(), "=ROUND([price]*[quantity],2)");
    assert.equal(await double.locator("..").getByText("Required", { exact: true }).count(), 0);
    assert.equal(await double.getByLabel("Calculation preview result").innerText(), "20");
    assert(!(await double.getByLabel("Value to multiply 1 source").locator("option").allTextContents()).some((s) => s.includes("first_name")));
    await double.getByLabel("Multiply by 2 source").selectOption("field:quantity");
    assert.equal(await double.getByLabel("Calculation preview result").innerText(), "100");
    await double.getByLabel("Preview values").selectOption(record.id);
    assert.equal(await double.getByLabel("Calculation preview result").innerText(), "73.5");
    await double.getByLabel("Preview quantity").fill("4");
    assert.equal(await double.getByLabel("Calculation preview result").innerText(), "98");
    const untouched = await (await page.request.get(base + `/api/v1/collections/${id}/documents/${record.id}`)).json();
    assert.equal(untouched.body.quantity, 3);
    await double.screenshot({ path: "/tmp/vulse-calculation-builder-detail.png" });
    await page.screenshot({ path: "/tmp/vulse-calculation-builder.png", fullPage: true });
    await double.getByLabel("Calculation", { exact: true }).selectOption("percentage");
    await double.getByLabel("Part 1 source").selectOption("field:price");
    await double.getByLabel("Total 2 source").selectOption("field:quantity");
    await double.getByLabel("Preview price").fill("25");
    await double.getByLabel("Preview quantity").fill("100");
    assert.equal(await double.getByLabel("Calculation preview result").innerText(), "25");
    await double.getByLabel("Preview quantity").fill("0");
    await double.getByText(/Cannot divide by zero/).waitFor();
    assert.equal(await double.getByText(/Formula syntax and field references are valid/).count(), 1);
    assert.equal(await double.getByLabel("Calculation preview result").count(), 0);
    await double.getByLabel("Calculation", { exact: true }).selectOption("TEXTJOIN");
    await double.getByLabel("Text to combine 3 source").selectOption("field:first_name");
    await double.getByLabel("Text to combine 4 source").selectOption("field:last_name");
    await double.getByLabel("Preview values").selectOption(record.id);
    assert.equal(await double.getByLabel("Calculation preview result").innerText(), "Ada Lovelace");
    await double.getByLabel("Calculation", { exact: true }).selectOption("IF");
    await double.getByLabel("Condition 1 source").selectOption("field:price");
    await double.getByLabel("Condition 1 comparison", { exact: true }).selectOption(">");
    await double.getByLabel("Condition 1 comparison value").fill("100");
    await double.getByLabel("When true 2 fixed value").fill("Premium");
    await double.getByLabel("Otherwise 3 fixed value").fill("Standard");
    assert.equal(await double.getByLabel("Calculation preview result").innerText(), "Standard");
    await double.getByLabel("Preview price").fill("120");
    assert.equal(await double.getByLabel("Calculation preview result").innerText(), "Premium");
    await double.getByLabel("Calculation", { exact: true }).selectOption("more");
    await double.getByLabel("Search functions").fill("round decimal");
    await double.getByRole("button", { name: /^ROUND Round a number/ }).click();
    await double.getByRole("button", { name: "Use ROUND", exact: true }).click();
    await double.getByLabel("Value to round 1 source").selectOption("field:price");
    await double.getByLabel("Preview values").selectOption(record.id);
    assert.equal(await double.getByLabel("Calculation preview result").innerText(), "24.5");
    await double.getByRole("button", { name: "Edit formula", exact: true }).click();
    await double.getByLabel("Formula", { exact: true }).fill("=[pr");
    await double.getByRole("option", { name: /^price \[price\]\s*Number$/ }).click();
    assert.equal(await double.getByLabel("Formula", { exact: true }).inputValue(), "=[price]");
    // Function names complete from the keyboard and show help for each argument.
    const formulaBox = double.getByLabel("Formula", { exact: true });
    await formulaBox.fill("=ro");
    await double.getByRole("listbox", { name: "Function suggestions" }).waitFor();
    assert.match(await double.getByRole("option").first().innerText(), /^ROUND\(number, digits\)/);
    await formulaBox.press("ArrowDown");
    await formulaBox.press("ArrowUp");
    await formulaBox.press("Enter");
    assert.equal(await formulaBox.inputValue(), "=ROUND(");
    const help = double.getByLabel("Function help");
    assert.match(await help.innerText(), /ROUND\(number, digits\)[\s\S]*Argument 1: Value to round/);
    await formulaBox.pressSequentially("[price], ");
    assert.match(await help.innerText(), /Argument 2: Decimal places/);
    assert.match(await double.locator(".formula-status").innerText(), /expected/);
    await formulaBox.pressSequentially("2)");
    assert.equal(await double.locator(".formula-status").innerText(), "\u2713 Formula is valid");
    await formulaBox.fill("=[price] * [nope]");
    assert.equal(await double.locator(".formula-status").innerText(), "Unknown field [nope]");
    await formulaBox.press("Escape");
    await double.getByLabel("Formula", { exact: true }).fill("=ROUND([price] * [quantity], 2) + 5");
    await double.getByLabel("Formula", { exact: true }).evaluate((area) => {
      const start = area.value.indexOf("[price]");
      area.focus();
      area.setSelectionRange(start, start + "[price]".length);
      area.dispatchEvent(new Event("select", { bubbles: true }));
    });
    await double.getByRole("button", { name: "Browse functions", exact: true }).click();
    await double.getByLabel("Search functions").fill("absolute value");
    await double.getByRole("button", { name: /^ABS Remove the sign/ }).click();
    await double.getByRole("button", { name: "Insert function at cursor", exact: true }).click();
    assert.equal(await double.getByLabel("Formula", { exact: true }).inputValue(), "=ROUND(ABS([price]) * [quantity], 2) + 5");
    assert.equal(await double.getByLabel("Calculation preview result").innerText(), "78.5");
    await page.getByRole("button", { name: "Save fields", exact: true }).click();
    await page.getByText("Changes saved", { exact: true }).waitFor();
    await page.reload();
    await page.getByRole("button", { name: "Fields", exact: true }).click();
    await double.getByRole("button", { name: "Edit formula", exact: true }).click();
    await double.getByLabel("Formula", { exact: true }).waitFor();
    assert.equal(await double.getByLabel("Formula", { exact: true }).inputValue(), "=ROUND(ABS([price]) * [quantity], 2) + 5");
    await total.getByRole("button", { name: "Edit formula", exact: true }).click();
    assert.equal(await total.getByLabel("Formula", { exact: true }).inputValue(), "=ROUND([price]*[quantity],2)");
    const saved = await (await page.request.get(base + `/api/v1/collections/${id}/documents/${record.id}`)).json();
    assert.equal(saved.body.double, 78.5);
    assert.equal(saved.body.quantity, 3);
    await page.setViewportSize({ width: 390, height: 844 });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth));
    await page.screenshot({ path: "/tmp/vulse-calculation-mobile.png", fullPage: true });
    await page.getByRole("button", { name: "Add field", exact: true }).click();
    const added = page.locator(".field-row").last();
    await added.getByLabel("Name", { exact: true }).fill("new_result");
    await added.getByLabel("Type", { exact: true }).selectOption("calculated");
    const fresh = page.getByRole("region", { name: "new_result calculation", exact: true });
    assert.equal(await fresh.getByLabel("Calculation", { exact: true }).inputValue(), "multiply");
    assert.equal(await fresh.getByLabel("Value to multiply 1 source").inputValue(), "");
    assert.equal(await fresh.getByLabel("Calculation preview result").count(), 0);
    await fresh.getByLabel("Value to multiply 1 source").selectOption("field:price");
    assert.equal(await fresh.getByLabel("Calculation preview result").innerText(), "20");
    await page.getByRole("button", { name: "Save fields", exact: true }).click();
    await page.getByText("Changes saved", { exact: true }).waitFor();
    const final = await (await page.request.get(base + `/api/v1/collections/${id}/documents/${record.id}`)).json();
    assert.equal(final.body.new_result, 49);
    // Collection summary formulas use the same recursive guided builder.
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.getByRole("button", { name: "Records", exact: true }).click();
    await page.getByRole("button", { name: "Summary", exact: true }).click();
    await page.getByRole("button", { name: "+ Add formula", exact: true }).click();
    const summary = page.getByRole("region", { name: "value_2 calculation", exact: true });
    await summary.getByRole("button", { name: "Start a new guided calculation", exact: true }).click();
    await summary.getByLabel("Calculation", { exact: true }).selectOption("ROUND");
    await summary.getByLabel("Value to round 1 source").selectOption("calculation");
    const nested = summary.getByRole("group", { name: "Value to round 1 nested calculation", exact: true });
    await nested.getByLabel("Calculation", { exact: true }).selectOption("divide");
    const sources = await nested.getByLabel("Value to divide 1 source").locator("option").allTextContents();
    assert(sources.some(s => s.includes("total_price")));
    assert(!sources.some(s => /\[price\]|\[value_2\]/.test(s)));
    await nested.getByLabel("Value to divide 1 source").selectOption("field:total_price");
    await nested.getByLabel("Divide by 2 source").selectOption("number");
    await nested.getByLabel("Divide by 2 fixed value").fill("3");
    await summary.getByLabel("Decimal places 2 fixed value").fill("2");
    await page.locator(".summary-results tbody").getByRole("cell", { name: "8.17", exact: true }).waitFor();
    await summary.getByRole("button", { name: "Edit formula", exact: true }).click();
    assert.equal(await summary.getByLabel("Value 2 formula", { exact: true }).inputValue(), "=ROUND(([total_price] / 3), 2)");
    await summary.getByRole("button", { name: "Use guided builder", exact: true }).click();
    assert.equal(await nested.getByLabel("Divide by 2 fixed value").inputValue(), "3");
    await page.getByRole("button", { name: "Save view…", exact: true }).click();
    await page.getByLabel("View name", { exact: true }).fill("Guided summary");
    await page.getByRole("button", { name: "Save view", exact: true }).click();
    await page.getByRole("button", { name: "Edit view…", exact: true }).waitFor();
    await page.reload();
    await page.getByLabel("Collection view", { exact: true }).selectOption({ label: "Guided summary" });
    await summary.getByLabel("Value 2 formula", { exact: true }).waitFor();
    assert.equal(await summary.getByLabel("Value 2 formula", { exact: true }).inputValue(), "=ROUND(([total_price] / 3), 2)");
    await summary.getByRole("button", { name: "Use guided builder", exact: true }).click();
    assert.equal(await nested.getByLabel("Divide by 2 fixed value").inputValue(), "3");
    await page.locator(".summary-results tbody").getByRole("cell", { name: "8.17", exact: true }).waitFor();
    await page.setViewportSize({ width: 390, height: 844 });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth));
    await summary.screenshot({ path: "/tmp/vulse-summary-guided-mobile.png" });
    // Cross-collection calculations: preserve the user's guarded SUMIFS formula,
    // then add math without turning missing matches into numeric results.
    await page.setViewportSize({ width: 1440, height: 1000 });
    const reportName = "aarsmelding-" + run;
    const sourceResponse = await page.request.post(base + "/api/v1/collections", { data: {
      name: reportName, visibility: "private", blueprint: { fields: [
        { name: "org_nummer", type: "string" }, { name: "aar", type: "number" },
        { name: "aarsverk_ex_spesped", type: "number" }
      ] }
    } });
    assert.equal(sourceResponse.status(), 201);
    const reports = await sourceResponse.json();
    for (const [org_nummer, aar, aarsverk_ex_spesped] of [["positive", 2025, 4.2], ["positive", 2025, 8.3], ["zero", 2025, 0], ["other-year", 2024, 7]]) {
      const response = await page.request.post(base + `/api/v1/collections/${reports.id}/documents`, { data: { body: { org_nummer, aar, aarsverk_ex_spesped } } });
      assert.equal(response.status(), 201);
    }
    const guardedSum = `=IF(COUNTIFS([${reportName}.org_nummer]; [orgnummer]; [${reportName}.aar]; [aar]) = 0; ""; SUMIFS([${reportName}.aarsverk_ex_spesped]; [${reportName}.org_nummer]; [orgnummer]; [${reportName}.aar]; [aar]))`;
    const firstMatch = `=XLOOKUP([orgnummer], [${reportName}.org_nummer], [${reportName}.aarsverk_ex_spesped])`;
    const lookupResponse = await page.request.post(base + "/api/v1/collections", { data: {
      name: "lookup-calculations-" + run, visibility: "private", blueprint: { fields: [
        { name: "orgnummer", type: "string" }, { name: "aar", type: "number" }, { name: "factor", type: "number" },
        { name: "guarded_sum", type: "calculated", formula: guardedSum },
        { name: "first_match", type: "calculated", formula: firstMatch }
      ] }
    } });
    assert.equal(lookupResponse.status(), 201);
    const lookupCollection = await lookupResponse.json();
    const lookupRecords = {};
    for (const orgnummer of ["positive", "zero", "missing", "other-year"]) {
      const response = await page.request.post(base + `/api/v1/collections/${lookupCollection.id}/documents`, { data: { body: { orgnummer, aar: 2025, factor: 2 } } });
      assert.equal(response.status(), 201);
      lookupRecords[orgnummer] = await response.json();
    }
    assert.equal(lookupRecords.positive.body.guarded_sum, 12.5);
    assert.equal(lookupRecords.zero.body.guarded_sum, 0);
    assert.equal(lookupRecords.missing.body.guarded_sum, "");
    assert.equal(lookupRecords["other-year"].body.guarded_sum, "");
    await page.goto(base + `/collections/${lookupCollection.id}`);
    await page.getByRole("button", { name: "Fields", exact: true }).click();
    const sumLookup = page.getByRole("region", { name: "guarded_sum calculation", exact: true });
    const firstLookup = page.getByRole("region", { name: "first_match calculation", exact: true });
    assert.equal(await sumLookup.getByLabel("Calculation", { exact: true }).inputValue(), "lookup");
    assert.equal(await sumLookup.getByLabel("Lookup calculation", { exact: true }).inputValue(), "SUMIFS");
    assert.equal(await sumLookup.getByLabel("Condition 2 value source", { exact: true }).inputValue(), "field:aar");
    assert(await sumLookup.getByLabel("Return empty text when no records match", { exact: true }).isChecked());
    await sumLookup.getByRole("button", { name: "Edit formula", exact: true }).click();
    assert.equal(await sumLookup.getByLabel("Formula", { exact: true }).inputValue(), guardedSum);
    await sumLookup.getByRole("button", { name: "Use guided builder", exact: true }).click();
    await sumLookup.getByLabel("Return empty text when no records match", { exact: true }).uncheck();
    assert((await sumLookup.locator(".calculation-formula code").innerText()).startsWith("=SUMIFS("));
    await sumLookup.getByLabel("Return empty text when no records match", { exact: true }).check();
    await sumLookup.getByRole("button", { name: "+ Add math step", exact: true }).click();
    await sumLookup.getByLabel("Math step 1 source", { exact: true }).selectOption("field:factor");
    const mathSources = await sumLookup.getByLabel("Math step 1 source", { exact: true }).locator("option").allTextContents();
    assert(!mathSources.some(s => /\[orgnummer\]|\[guarded_sum\]/.test(s)));
    await sumLookup.getByRole("button", { name: "+ Add math step", exact: true }).click();
    await sumLookup.getByLabel("Math step 2 operation", { exact: true }).selectOption("+");
    await sumLookup.getByLabel("Math step 2 number", { exact: true }).fill("1");
    await sumLookup.getByRole("button", { name: "+ Add math step", exact: true }).click();
    await sumLookup.getByLabel("Math step 3 operation", { exact: true }).selectOption("/");
    await sumLookup.getByLabel("Math step 3 number", { exact: true }).fill("3");
    await sumLookup.getByRole("button", { name: "+ Add math step", exact: true }).click();
    await sumLookup.getByLabel("Math step 4 operation", { exact: true }).selectOption("ROUND");
    await sumLookup.getByLabel("Math step 4 number", { exact: true }).fill("");
    await sumLookup.getByText("Math step 4: choose a numeric field or enter a number.", { exact: true }).waitFor();
    await sumLookup.getByLabel("Math step 4 number", { exact: true }).fill("2");
    await firstLookup.getByLabel("Return empty text when no records match", { exact: true }).check();
    await firstLookup.getByRole("button", { name: "+ Add math step", exact: true }).click();
    await page.getByRole("button", { name: "Save fields", exact: true }).click();
    await page.getByText("Changes saved", { exact: true }).waitFor();
    for (const [org, expected] of [["positive", 8.67], ["zero", 0.33], ["missing", ""], ["other-year", ""]]) {
      const response = await page.request.get(base + `/api/v1/collections/${lookupCollection.id}/documents/${lookupRecords[org].id}`);
      const saved = await response.json();
      assert.equal(saved.body.guarded_sum, expected, org);
      assert.equal(saved.body.first_match, org === "missing" ? "" : lookupRecords[org].body.first_match * 2, org);
    }
    await page.reload();
    await page.getByRole("button", { name: "Fields", exact: true }).click();
    assert.equal(await sumLookup.getByLabel("Calculation", { exact: true }).inputValue(), "lookup");
    assert(await sumLookup.getByLabel("Return empty text when no records match", { exact: true }).isChecked());
    assert.equal(await sumLookup.getByLabel("Math step 1 source", { exact: true }).inputValue(), "field:factor");
    assert.equal(await sumLookup.getByLabel("Math step 4 operation", { exact: true }).inputValue(), "ROUND");
    assert.equal(await firstLookup.getByLabel("Math step 1 number", { exact: true }).inputValue(), "2");
    await sumLookup.screenshot({ path: "/tmp/vulse-lookup-math-desktop.png" });
    await sumLookup.getByRole("button", { name: "Remove math step 2", exact: true }).click();
    assert.equal(await sumLookup.getByLabel("Math step 2 operation", { exact: true }).inputValue(), "/");
    assert.equal(await sumLookup.getByLabel("Math step 3 operation", { exact: true }).inputValue(), "ROUND");
    await page.getByRole("button", { name: "Save fields", exact: true }).click();
    await page.getByText("Changes saved", { exact: true }).waitFor();
    for (const [org, expected] of [["positive", 8.33], ["zero", 0], ["missing", ""]]) {
      const response = await page.request.get(base + `/api/v1/collections/${lookupCollection.id}/documents/${lookupRecords[org].id}`);
      assert.equal((await response.json()).body.guarded_sum, expected, org);
    }
    await page.setViewportSize({ width: 390, height: 844 });
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth));
    await sumLookup.screenshot({ path: "/tmp/vulse-lookup-math-mobile.png" });
    assert.deepEqual(errors, []);
    console.log("PASS: existing formula recognition/preservation, typed guided inputs, sample and record previews without writes, percentages/runtime errors, text joining, conditions, function search/help, field autocomplete, cursor insertion, save/reload/server results, nested calculations, collection summary wizard and saved view persistence, lookup math steps, empty versus zero matching results, mobile layout; no runtime errors");
  } catch (error) {
    await page.screenshot({ path: "/tmp/vulse-calculation-failure.png", fullPage: true });
    console.error("Browser errors:", errors);
    throw error;
  } finally {
    await browser.close();
  }
})();
