# Mini talk (fixture)

Preamble paragraph with a [link](https://example.com) and `code`.

---

## Takeaways

1. **First takeaway.** Something to remember.
2. Second, with a nested list:
   - nested one;
   - nested two.

   A continuation paragraph inside item 2.

## Run of show

| Time | Part |
|---|---|
| 0:00–2:00 | Opening |
| 2:00–10:00 | Build |

## 0:00–2:00 · Opening

Hello and welcome. This is **the opening**, with `inline code` and a
[link that should lose its URL](https://example.com/very/long).

A second spoken paragraph in the same beat.

<details>
<summary>Presenter notes</summary>

- Have the terminal open.
- Breathe.
</details>

## 2:00–10:00 · Build

### Step 1 · Make a table · 2:00–4:00

**Create it in the console.** Choose **Create**.

Some more about the console, same beat.

**Now in psql.** From pane B:

```sql
CREATE TABLE t (id int PRIMARY KEY);
```
```
CREATE TABLE
```

> **★ DSQL difference: keys matter.** The primary key is the layout.

> No agent? Run `aws thing create` instead:
> ```sh
> aws thing create --name x
> ```

Then ask the agent:

```prompt
Build me an API for table t
using the cluster <cluster-endpoint>.
```

<!-- beat -->

...and that was the wrong cluster.

### Steps 2 and 3 · Race and load · 4:00–8:00

```sql
-- both panes, one line at a time
SELECT 1 AS x \gset
SELECT 2 AS y \gset
```

| | Pane A | Pane B |
|---|---|---|
| 1 | `BEGIN;` | |
| 2 | | `BEGIN;` |
| 3 | `COMMIT;` → ok | |
| 4 | | `COMMIT;` → `ERROR: conflict (OC000)` |

Look at the table:

```
 id | name
----+------
  1 | a
```

In a second terminal:

```sh
./load.sh \
  --rows 10
echo done
```

<details>
<summary>Following along: do it yourself</summary>

```sh
./setup.sh
```
</details>

### Step 4 · Wrap the build · 8:00–10:00

| We did | It was different |
|---|---|
| Made a table | Keys are layout |
| Raced | Optimistic |

## Cleaning up

```sh
./destroy.sh
```
