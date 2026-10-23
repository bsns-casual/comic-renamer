# Kavita, researched from source

Date: 2026-10-23
Source: `Kareadita/Kavita`, branch `develop`, read via the GitHub API.
Method: the same as §6.8.1 for Komga — source, not documentation. Secondary
sources were consulted first and **contradicted each other** on the one
question that matters (filename versus folder precedence), which is why this
document exists.

Files read: `Kavita.Services/Scanner/Parser.cs`,
`BasicParser.cs`, `PdfParser.cs`, `BookParser.cs`,
`Kavita.Services/Reading/ReadingItemService.cs`,
`Kavita.Models/Constants/ParserConstants.cs`,
`Kavita.Services/Helpers/PdfComicInfoExtractor.cs`.

---

## 1. The headline: Kavita is filename-driven, Komga is folder-driven

They are opposites, and our Komga layout is actively wrong for Kavita.

| | Komga (§6.8.1) | Kavita |
|---|---|---|
| Series name from | **leaf folder only**; ancestors never read | **filename**; folder only as a fallback |
| Filename is | the displayed book title | the series *and* the title source |
| Our flat `<Series>/<Book>.<ext>` | correct by design | fragments the library — see §3 |

`BasicParser.Parse` and `PdfParser.Parse` both do:

```csharp
ret.Series = Parser.ParseSeries(fileName, type);
if (ret.Series == string.Empty)
{
    // Try to parse information out of each folder all the way to rootPath
    ParseFromFallbackFolders(filePath, rootPath, type, ref ret);
}
```

The folder is consulted **only when the filename yields an empty series.**

## 2. Which parser handles a PDF

`ReadingItemService` selects in a fixed order:

```csharp
if (_comicVineParser.IsApplicable(path, type)) return _comicVineParser;
if (_imageParser.IsApplicable(path, type))     return _imageParser;
if (_bookParser.IsApplicable(path, type))      return _bookParser;
if (_pdfParser.IsApplicable(path, type))       return _pdfParser;
if (_basicParser.IsApplicable(path, type))     return _basicParser;
```

`BookParser.IsApplicable` is `Parser.IsEpub(filePath)` — EPUB only.
`PdfParser.IsApplicable` is `Parser.IsPdf(filePath)`.

**So every PDF goes to `PdfParser`, in every library type but ComicVine.**
`BasicParser` never sees a PDF. That distinction decides §4.

## 3. The real blocker: series fragmentation, not Specials

`ComicSeriesRegex` is 22 patterns, and the last is a catch-all with an explicit
comment:

```csharp
// MUST BE LAST: Batman & Daredevil - King of New York
new Regex(@"^(?<Series>.*)", MatchOptions, RegexTimeout)
```

**The catch-all means `ParseComicSeries` never returns empty, so for a Comic
library the folder name is never read at all.** The fallback in §1 is
unreachable.

Measured, running all 22 extracted patterns in order against filenames our
Komga preset actually emits:

```
filename (as our Komga preset emits it)  Kavita series                        vol  catch-all
Hack - Slash - Back to School            'Hack - Slash - Back to School'      -    YES
Usagi Yojimbo - The Special Edition      'Usagi Yojimbo - The Special Edition' -   YES
Young Shadow - The Watchdogs             'Young Shadow - The Watchdogs'       -    YES
The Spawn of Venus and Other Stories     'The Spawn of Venus and Other Stories' -  YES
Killadelphia Vol. 04 - The End of All    'Killadelphia '                      04   no
Moonshine Vol. 01                        'Moonshine '                         01   no
'68 Vol. 06 - Last Rites                 "'68 "                               06   no
DIE - Loaded #1                          'DIE - Loaded'                       -    no
Minotaur #1                              'Minotaur'                           -    no
```

A volume marker saves the row: `Killadelphia Vol. 04 - The End of All` parses
to series *Killadelphia*, volume 4, which is correct. Without one, the
catch-all makes **the whole filename the series name**, so every volumeless
book becomes its own single-book series.

Share of the corpus that has no volume marker:

```
Humble ground truth, 2223 rows
  with a volume marker :  1333   60.0%   series parses correctly
  without              :   890   40.0%   catch-all -> own single-book series
```

**40% of a real bundle would land as 890 separate one-book series.**

Caveat: two of the ten `ComicVolumeRegex` patterns use .NET constructs that do
not compile in Python, so the volume column is a lower bound. All 22 series
patterns compiled, so the series column is complete.

## 4. Specials — the existing backlog claim is wrong for Comic libraries

§8's backlog says Kavita "files every file with no issue number as a Special"
and treats PDF→CBZ conversion as load-bearing because of it. From source, that
is **true only for a Book library.**

`PdfParser` gates the automatic rule on library type:

```csharp
if (Parser.IsDefaultChapter(ret.Chapters) && Parser.IsLooseLeafVolume(ret.Volumes)
    && type == LibraryType.Book)
{
    ret.IsSpecial = true;
```

`BasicParser` has the same rule **ungated** — but per §2 it never receives a
PDF, so that ungated path is unreachable for our output.

The other two routes to Special are narrow:

```csharp
public static bool IsSpecial(string? filePath, LibraryType type)
{
    return HasSpecialMarker(filePath);
}
```

`IsSpecial` is *only* a marker check — there is no keyword list. Our filenames
carry no marker.

**So a PDF in a Comic or Manga library is never auto-Special from our output.**
Specials land in reserved volume `100000`
(`ParserConstants.SpecialVolumeNumber`); loose-leaf volumes use `-100000`.

Not verified: how a loose-leaf volume renders in the Kavita UI. "Not a
Special" is not the same claim as "shelves tidily", and the 890 rows above are
loose-leaf as well as fragmented.

## 5. Kavita reads embedded PDF metadata, and that is a way out

`PdfComicInfoExtractor` translates PDF document-information-dictionary and XMP
keys into a `ComicInfo`, which `UpdateFromComicInfo` then patches over anything
parsed from the filename:

| PDF key | ComicInfo field |
|---|---|
| `Series` | `Series` — falls back to `Title` when absent |
| `Volume` | `Volume` |
| `Title` | `Title` |
| `TitleSort` | `TitleSort` |
| `Summary` | `Summary` |
| `Publisher` | `Publisher` |
| `Author` | `Writer` |
| `Subject` | `Genre` |
| `Language` | `LanguageISO` |
| `ISBN` | `Isbn` |
| `CreationDate` | publication date |
| `UserRating` | `UserRating` |

`Series`, `Volume`, `TitleSort` and `ISBN` are **custom** Info-dictionary keys,
not standard PDF ones, so any PDF writer can add them.

This matters twice over:

1. It fixes §3 completely. A `Series` key of `Hack/Slash` gives Kavita the real
   series name — slash included — with no filesystem constraint, and a
   `Volume` key fixes the 40%.
2. It is the first concrete payoff for the PDF-metadata backlog item, and it
   means that item is a Kavita *enabler*, not a nicety.

## 6. What this means for the project

- **Phase 2's Kavita preset cannot be the Komga preset with a different path
  template.** Komga wants the series in the folder; Kavita wants it in the
  filename. That is a genuine divergence in the preset abstraction, and §8's
  Phase 2 exit criterion ("the preset abstraction survives every app the spike
  validated") is the right place to test it.
- A Kavita-shaped filename would need the series repeated in the filename with
  a parseable volume — roughly `<Series>/<Series> Vol. NN - <Subtitle>.pdf`,
  which is close to what we already emit *when a volume exists*. The volumeless
  40% is the open design problem.
- **§8's backlog entry for PDF→CBZ should be rewritten.** Its stated
  justification does not hold for Comic libraries. There may still be reasons
  to convert, but "every file becomes a Special" is not one of them.
- Writing PDF metadata is the cheaper and more direct fix for both problems.

## 7. Not established

- Loose-leaf rendering in the UI.
- `LibraryType.ComicVine` behaviour, which short-circuits before `PdfParser`.
- Whether `MangaSeriesRegex` has an equivalent catch-all; the §3 finding is
  scoped to Comic and ComicVine, which both use `ParseComicSeries`.
- Any of this against a running Kavita instance. Everything here is read from
  source on `develop` and should be confirmed against a real library before
  Phase 2 commits to a preset shape.
