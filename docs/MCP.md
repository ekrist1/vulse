# Connecting Claude and other AI assistants (MCP)

Vulse includes an [MCP](https://modelcontextprotocol.io) server. Once an admin turns it on, you can connect Claude (or any other MCP client) to your workspace and ask it to find, summarize, add and change records in plain language: "Which orders over 1000 are still open?", "Add these five contacts", "Mark every task tagged *launch* as done".

The assistant works with the same permissions you have. It can read and write **records** only. It cannot change collection fields, people, API keys, imports or files.

- [For admins](#for-admins): turning it on and controlling access
- [For users](#for-users): connecting Claude and using it
- [Tool reference](#tool-reference)

## For admins

### Turning it on

Set two variables and restart Vulse:

| Variable | Value |
| --- | --- |
| `VULSE_MCP` | `on` |
| `VULSE_PUBLIC_URL` | The address people use to reach Vulse, such as `https://cms.example.com` |

`VULSE_PUBLIC_URL` must use `https://`. Vulse refuses to start otherwise, except for `localhost` during development. The address must not have a path: Vulse has to be served at the root of its host, because clients look for its sign-in details at `/.well-known/…`. Without `VULSE_PUBLIC_URL`, Vulse logs a warning and assumes `http://` plus the `BIND` address, which only works on the same machine.

When it is on, Vulse serves these addresses in addition to the admin and `/api/v1`:

| Path | Purpose |
| --- | --- |
| `/mcp` | The MCP server. Give this address to users. |
| `/.well-known/oauth-protected-resource`, `/.well-known/oauth-protected-resource/mcp`, `/.well-known/oauth-authorization-server` | Lets clients discover how to sign in |
| `/oauth/register`, `/oauth/authorize`, `/oauth/token` | Sign-in for MCP clients (OAuth 2.1) |
| `/oauth/consent` | The page where users approve a client |

**Behind a reverse proxy**, route all of these paths to Vulse. Pass the `Authorization` header through and keep the original `Host` header: `/mcp` only accepts requests addressed to `localhost` or to the host in `VULSE_PUBLIC_URL`, which protects against DNS rebinding. MCP responses are plain JSON, so response buffering is fine. Set `VULSE_TRUST_PROXY=true` (see [Sign-in limits](../README.md#sign-in-limits)) so rate limits apply per client rather than to the proxy.

To turn MCP off again, remove `VULSE_MCP` and restart. Connected apps stop working immediately. Their approvals stay stored and work again if you turn MCP back on, unless they have expired.

### Who can do what

Users connect their own Claude. Nothing in the workspace is reachable until a signed-in user approves the connection on the consent page, and the connection then acts as that user.

| User's role | Access they can grant | What the assistant can do |
| --- | --- | --- |
| Viewer | Only read records | Read collections and records |
| Editor or Admin | Read and change records, or only read | Also create, update, delete and undo records |

- Admin rights are never granted to an assistant, so even an admin's connection cannot change schemas, people or keys.
- When approving, a user can limit the connection to **one collection**.
- If you lower a user's role, their connections drop to the new role at the next request.
- By default any MCP client can introduce itself to Vulse, so users can connect without your help. That grants nothing until a user approves it. To decide yourself which apps may connect, see [Choosing which apps can connect](#choosing-which-apps-can-connect).

API keys also work with MCP (see [Connecting with an API key](#connecting-with-an-api-key)). A key's role and collection limit apply there too.

### Choosing which apps can connect

Apps introduce themselves to Vulse in one of two ways:

- **Registered**: the app registers when someone first connects it (OAuth dynamic client registration). Each registration is a separate entry, often one per user.
- **Verified domain**: the app's id is the address of a description it publishes on its own website, such as `https://claude.ai/…` (Client ID Metadata Document). Vulse reads that description, at most once a day, and shows its domain on the consent page. One entry covers everyone using the app.

Under **API keys → Apps that can connect**, admins see every app Vulse knows: its name, where it returns users after sign-in or its verified domain, and how many people are connected through it.

**Let any app connect once a user approves it** is on by default. Turn it off to approve apps yourself:

- A new app is listed as *waiting for approval*. Anyone who tries to connect it sees a page saying an admin must approve it first. Approve it, and they connect again.
- Apps that were already allowed, and everyone connected through them, keep working.
- Approving an app with a verified domain once allows it for the whole workspace.
- **Remove** disconnects everyone using an app. If it asks to connect again while the setting is off, it needs approval again.

Unused apps that nobody approved are cleaned up automatically: after a day when the setting is on, after a week while they wait for approval. Apps an admin approved stay until they are removed.

Vulse only reads app descriptions from public `https` addresses with a domain name. It never contacts addresses on your own network, follows no redirects, and gives up after 5 seconds or 64 KB. If an app's website is down, Vulse keeps using the description it read last.

### Seeing and revoking connections

Open **API keys**. As an admin, **Connected apps in the workspace** lists every connection with its owner, its access and when it was last active. **Disconnect** ends a connection immediately.

These also disconnect all of a user's apps:

- disabling the user
- a password reset or change
- the user choosing **Log out everywhere**

### Auditing changes

Changes an assistant makes appear in each record's **Revision history**, marked **via AI assistant (MCP)**. Deleted records go to the collection's **Trash** and can be restored there. Revision history does not record which person made a change, for assistants or for anyone else. Every change an assistant makes can also be undone for 24 hours (see [Undoing changes](#undoing-changes)).

### Sessions and tokens

| | Lifetime |
| --- | --- |
| Consent page link | 10 minutes |
| Access token | 1 hour |
| Refresh token | 30 days, replaced each time it is used |

An app that is used at least once a month stays connected. Codes and tokens are stored only as hashes. If a refresh token or sign-in code is presented twice, Vulse treats it as stolen and ends that connection; the user then connects again.

## For users

### Connecting Claude (claude.ai and Claude Desktop)

Ask your admin for the Vulse address, or find it under **API keys → Connected apps**. It ends in `/mcp`, such as `https://cms.example.com/mcp`.

1. In Claude, open **Settings → Connectors** and choose **Add custom connector**.
2. Name it *Vulse* and paste the address. Leave the advanced OAuth fields empty: Claude registers itself with Vulse.
3. Choose **Connect**. A Vulse window opens. Sign in if asked.
4. On the consent page, choose what Claude may do (see [Choosing access](#choosing-access)) and select **Allow**.

You return to Claude, and Vulse is listed under your connectors. Connectors added on claude.ai are also available in Claude Desktop. On Team and Enterprise plans, an organization owner adds the connector under **Organization settings → Connectors**, and each member then selects **Connect** to approve it for their own Vulse account.

### Connecting Claude Code

```sh
claude mcp add --transport http vulse https://cms.example.com/mcp
```

Then run `/mcp` in Claude Code, select *vulse* and authenticate. Your browser opens the Vulse consent page.

### Connecting with an API key

Clients that cannot sign in through a browser, such as scripts or CI, can use an API key instead. Create one under **API keys** with the role and collection the client needs, then send it as a bearer token:

```sh
claude mcp add --transport http vulse https://cms.example.com/mcp \
  --header "Authorization: Bearer vulse_…"
```

Other MCP clients usually have a field for the server URL and either sign in automatically (OAuth) or accept a custom `Authorization` header.

### Choosing access

The consent page shows which app is asking and where you return afterwards. Only allow apps you started connecting yourself.

- **Access**: *Read and change records* lets the assistant create, update and delete records. *Only read records* lets it look but not change anything. Viewers can only grant read access.
- **Collections**: *All collections*, or one collection the assistant is limited to.

You can change your mind at any time: disconnect the app under **API keys → Connected apps** and connect it again with different choices.

### What to ask

The assistant first looks up which collections exist and what their fields are, so you can refer to things by their everyday names.

- "What collections do I have in Vulse?"
- "Show the ten most recent orders that are still open."
- "How many contacts have no email address?"
- "What is the total order value per customer this year?"
- "Find the task about the pricing page and set its status to done."
- "Add these people to the contacts collection: …" (paste a list)
- "Change the hours on all tasks assigned to Kim from 4 to 6."
- "Delete the test records I created today."
- "Undo that."

### How changes behave

- **All or nothing.** A request that creates or changes several records either succeeds for all of them or saves none. The assistant is told what was wrong, such as a missing required field or a value that is not one of a dropdown's options, and usually corrects it and tries again.
- **Calculated fields** are filled in by Vulse and cannot be set. Values an assistant supplies for them are ignored.
- **Conflicts.** If someone else changed a record after the assistant read it, the change can be refused rather than overwriting their work. Ask the assistant to read the record again.
- **Deleting** moves records to the collection's Trash. Records that other records still link to cannot be deleted. Vulse marks deleting and undoing as destructive, so Claude can ask you to confirm first, depending on your permission settings.
- At most 100 records are created, changed or deleted per request. The assistant splits larger jobs.

### Undoing changes

Every create, change or delete returns an undo reference to the assistant, so you can say "undo that" within 24 hours. Undo is refused if a record changed again afterwards; use the record's **Revision history** or the **Trash** instead.

### Disconnecting

Open **API keys** in Vulse and select **Disconnect** next to the app under **Connected apps**. You can also remove the connector in Claude, but that alone does not revoke the access Vulse granted.

### Troubleshooting

| Problem | What to do |
| --- | --- |
| Claude says it cannot connect, or the connector keeps asking you to connect again | The connection was disconnected, expired after 30 days without use, or ended because your password changed or you logged out everywhere. Connect again. Ask an admin whether MCP is turned on. |
| "Cannot connect to Vulse" page during sign-in | The app's registration expired or the link was changed. Start connecting again from the app. |
| "A workspace admin must approve … before it can connect" | Your admin decides which apps may connect. Ask them to approve it under **API keys → Apps that can connect**, then connect again. |
| "This sign-in request has expired" | The consent page was open for more than 10 minutes. Start again from the app. |
| "this action requires the editor role" | The connection has read-only access, or your role is Viewer. Disconnect and connect again with *Read and change records*, or ask an admin for the Editor role. |
| "this api key is limited to one collection" | The connection or key is limited to another collection. |
| A collection is missing from the assistant's list | Same as above, or the collection was created under another name. |
| Requests fail with `403 Forbidden: Host header is not allowed` | A proxy changes the `Host` header, or `VULSE_PUBLIC_URL` does not match the address used. See [Turning it on](#turning-it-on). |

## Tool reference

These are the tools Vulse offers to assistants. You don't call them yourself, but their names appear in Claude when it asks for permission.

| Tool | Access | What it does |
| --- | --- | --- |
| `list_collections` | read | Lists collections with their field names |
| `describe_collection` | read | Field types, required fields, allowed values, linked collections and formulas |
| `query_records` | read | Search text, filters (equals, contains, greater/less than, empty, combined with AND/OR), sorting and paging |
| `get_record` | read | One record by id |
| `run_sql` | read | One read-only [Vulse SQL](../README.md#sql) `SELECT`, for counts, sums, averages and grouping. Returns up to 200 rows. `UPDATE` and `DELETE` are refused |
| `create_records` | change | Creates 1–100 records |
| `update_records` | change | Changes chosen fields of 1–100 records, optionally only if unchanged since read |
| `delete_records` | change | Moves 1–100 records to the Trash |
| `undo_action` | change | Reverts one of the above changes made in the last 24 hours |

Collections can be named by name or id. Records are returned as `{id, fields, created_at, updated_at}`, with calculated values filled in.
