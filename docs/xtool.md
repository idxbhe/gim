# XTool Complete Documentation



# Main Index

XTool





[XTool](#main-index)



# Welcome to XTool

XTool is program made specifically repackaging games by providing a full suite of useful features such as data precompression, archiving, encryption and etc.

With that being said, nothing restricts it from being used on everyday files such as documents, pictures and media but with few limitations.

* [Precompressor](#precompressor)
* [Version history](#versionhistory)
* [Acknowledgement](#acknowledgement)

Last updated: 28.09.2021

[![Created with DA-HelpCreator](https://www.da-software.de/cwe_en.png "Created with DA-HelpCreator")](https://da-software.de/software/da-helpcreator/)


## Acknowledgement

XTool





[XTool](#main-index)



# Acknowledgement

#### [Fileforums community](https://fileforums.com/forumdisplay.php?f=55)

#### [Krinkels community](http://krinkels.org)

#### Tools used internally

* [BDiff](https://github.com/delphidabbler/bdiff)(Delphi translated) Peter
  D Johnson* [Fundamentals
    5 Library](https://github.com/fundamentalslib/fundamentals5)* [Lape](https://github.com/nielsAD/lape) Niels
      AD* [mORMot](https://github.com/synopse/mORMot) Synopse* [OpenCL](https://www.khronos.org/opencl/) Khronos
          Group, Dmitry 'skalogryz' Boyarintsev (for Delphi port)* [Parse
            Expression](http://www.sparcs-center.org/expression-parser) Egbert van Nes* [XDelta3](http://xdelta.org/)* [ZLib](https://zlib.net/) Jean-loup
                Gailly, Mark Adler

#### 

#### Tools used externally

* [GrittiBanzli](https://github.com/google/grittibanzli) Google * [LZ4](https://github.com/lz4/lz4) and [ZSTD](https://github.com/facebook/zstd) Yann
    Collet, Facebook* [LZO](http://www.oberhumer.com/opensource/lzo/) Markus
      F.X.J. Oberhumer* [Oodle](http://www.radgametools.com/oodle.htm) RAD
        Game Tools* [Preflate](https://github.com/deus-libri/preflate) Dirk
          Steinke* [Reflate](https://encode.su/threads/1399-reflate-a-new-universal-deflate-recompressor) Eugene
            Shelwien


## Precompressor

XTool





[XTool](#main-index)



# Precompressor

The precompressor XTool provides is built for speed and makes full use of the system's processing power.

****You can find out how to use it from [here](#precompressor-howtouseit)****

#### What makes it fast?

The precompressor is fast because it makes use of multi-threading for both scaning and processing streams found during the precompression process. (This is both when encoding and decoding data)

The precompression process is sequential meaning data scanning, processing, reading and writing to disk all occurs at virtually the same time.

It works entirely on system memory and only, reading and writing data to and from the disk only once.

Since the project is written in Delphi which has poor compiler optimisation compared to what other languages offer, intense manual code optimisation was done to ensure that the only bottleneck comes
from external libraries.

Lastly, it stores and makes use statistics based on data that it had already processed before to determine how best to process the data that follow.

#### Why does xtool comes with external libraries?

XTool does not include code of zlib, lz4 or other codecs mainly because it's written in Delphi and all these sources are written in C++.

Delphi does have a feature to import static libraries compiled by C/C++ but there are certain conditions that should be met before the Delphi compiler can allow them to be imported.

The most important reason is, most of codecs process streams via trial and error and compression libraries like lz4 and zstd each version compress data differently which affects the trial and error
method significantly and this way of allowing the user to provide their own external library gives them an option to pick a different library if they have issues with another.

If a certain compression library gets an update, zstd for example. The user can compile their own library then use it with XTool without waiting on the creator to update the program to support it.

External libraries are required both when encoding and decoding unless stated otherwise.


## Version_history

XTool





[XTool](#main-index)



# Version history

**ES\_R22 (0.3.22)**

- updated search support (speed improvements)

**ES\_R21 (0.3.21)**

- updated search support

**ES\_R20 (0.3.20)**
  
- fixed library support bug  
- x86 build discontinued (has bugs from nowhere)

**ES\_R19 (0.3.19)**
  
- updated lzo codec

**ES\_R18 (0.3.18)**
  
- fixed depth bug  
- fixed library plugin bugs

**ES\_R17 (0.3.17)**
  
- fixed multi-threading bug

**ES\_R16 (0.3.16)**
  
- minor bug fixes

**ES\_R15 (0.3.15)**
  
- converted library support to unicode (don't know why I used ansi in the first place)  
- added library support functions  
- added rc4 encryption support

**ES\_R14 (0.3.14)**
  
- fixed library support bug  
- updated library structure

**ES\_R13 (0.3.13)**
  
- updated lz4 codec  
- updated library structure  
- updated depth info functions  
- updated depth feature

**ES\_R12 (0.3.12)**
  
- added depth info functions  
- added support for oodle 2.9.0+ functions  
- fixed data patching bug  
- updated oodle codec  
- updated command line parser

**ES\_R11 (0.3.11)**
  
- fixed x86 build bugs  
- fixed config multi-threading bug  
- fixed resource management bug  
- fixed deduplication bug

**ES\_R10 (0.3.10)**
  
- added diff tolerance parameter (--diff=)  
- fixed plugin database bug  
- updated lz4 codec bug  
- updated oodle codec  
- updated library structure

**ES\_R9 (0.3.9)**- fixed future stream bug

**ES\_R8 (0.3.8)**- fixed command line parser bug  
- updated library support

**ES\_R7 (0.3.7)**
  
- updated library structure

**ES\_R6 (0.3.6)**
  
- updated oodle codec (fixed more lzna bugs)

**ES\_R5 (0.3.5)**
  
- updated oodle codec (fixed lzna bug)  
- added custom method configuration

**ES\_R4 (0.3.4)**

- fixed bug depthing

**ES\_R3 (0.3.3)**

- updated lz4 codec  
- updated library support

**ES\_R2 (0.3.2)**

- improved depthing  
- updated library support  
- fixed zstd codec issues  
- removed fast memory

**ES\_R1 (0.3.1)**

- updated library support  
- updated command line parser  
- included x86 build  
- fixed depthing issues

**2012\_R2 (0.2.14)**

- added library support  
- added compress, decompress, encrypt, decrypt, hash, delta functions (used by library)  
- added lzo codec placeholders  
- fixed oodle bug  
- fixed lz4 bug  
- removed
libdunia codec

**2012\_R1 (0.2.13)**

- added oo2ext\* dll support  
- updated search support

**2011\_R1 (0.2.12)**

- added temporary libdunia codec

**2010\_R5 (0.2.11)**

- fixed search/config support bug

**2010\_R4 (0.2.10)**

- updated search/config support

**2010\_R3 (0.2.9)**

- added database search  
- updated zlib scanner  
- fixed reflate bug  
- fixed 2GB memory limit

**2010\_R2 (0.2.8)**

- fixed zstd codec

**2010\_R1 (0.2.7)**

- added zstd codec  
- added lz4, lz4hc, lzna, mermaid, selkie, hydra, leviathan codec placeholders  
- added configuration support  
- added xdelta support to handle crc mismatch streams

**2009\_R3 (0.2.6)**

- documentation added

**2009\_R2 (0.2.5)**

- added kraken codec  
- fixed depthing issues

**2009\_R1 (0.2.4)**

- added reflate forced verification  
- updated deflate scanner  
- fixed depthing issues  
- fixed low memory mode issues  
- fixed hanging issues when encoding

**2008\_R3 (0.2.3)**

- fixed deduplication memory calculation error  
- added virtual memory support for deduplication  
- added --mem=# parameter to control deduplication memory usage

**2008\_R2 (0.2.2)**

- fixed command line parser  
- updated deflate scanner  
- added stream deduplication  
- added stream database  
- added decompression memory limiter  
- added grittibanzli (also handles deflate
stream but slow af)

**2008\_R1 (0.2.1)**

 - initial release


### Precompressor ❯ Codecs

XTool





[XTool](#main-index)



# Codecs

Codecs are the name of compression algorithms/strategies or imported tools to use on data. lz4 is an example, reflate is another example. lz4 in this case is a compression algorithm
and reflate is an imported tool.

Here is a list of internal codecs currently supported by XTool:

* [ZLib](#precompressor-codecs-zlib)* [LZ4](#precompressor-codecs-lz4)* [LZO](#precompressor-codecs-lzo)* [ZStd](#precompressor-codecs-zstd)* [Oodle](#precompressor-codecs-oodle)* [Encryption](#precompressor-codecs-encryption)

The word "internal" should tell you that there are also external codecs, these codecs are the ones a user creates by themselves in order to use them with XTool. The creation of external codecs should
be found [here](#precompressor-codecs-externalcodecs).


### Precompressor ❯ Deduplication

XTool





[XTool](#main-index)



# Deduplication

Yes, XTool comes with internal data deduplication. It's not really wide data deduplication like what rep/srep or exdupe do, it just focuses on the the streams it finds. So if it found no streams, it
will not be doing any deduplication.

#### How does it work?

Streams are sometimes found in repetitions therefore forcing the precompressor to process these streams more than once, what XTool does is only process one of the copies and store it elsewhere and
then where a repetition was found XTool just inserts it as a processed stream.

It sounds exactly like how srep works but there is a difference. These duplicated streams that are found are stored in memory and since precompression mostly inflates data (new data becomes
bigger than the original), the data if presented to srep ends up using more memory because these are not just duplicated streams that srep is processing but it's actually inflated duplicated streams. The
main advantage here is lowered memory usage and less IO being done to the disk and lastly, more speed because if for example there were 10 duplicated streams found in a certain data, only 1 of those
duplicates will be processed and mathematically, the process time should be a 10th.

#### Why not just use srep before precompression if data has repetitions?

This is not a bad idea to use data deduplication before you precompress the data because at this point, it would be smaller meaning less memory usage and less IO as the main goal of the internal data
deduplication. There is however one small problem. The search for streams is mainly done by the header information left behind by an archiver/compressor therefore using srep to remove duplicates messes
up with the data arrangement the scanner expects therefore affecting the precompression process leading into several streams not being found.

#### Does internal deduplication replace srep?

No, you still need to use srep because apart from duplicated streams, there still could be traces of duplicated non-stream data and the inflated data itself could also contain duplicates.

#### Why does XTool's deduplication feature produce an additional file?

This is simply because XTool is mostly used in StdIO mode, this means data can only be read and written once and seeking is not allowed therefore during decoding, the deduplication header information
should be obtained first before the actual decoding process and this is not possible without either seeking or creating temps.

The duplication file that is created is like a file that was created by an archiver which means, multiple datasets can rely on the same deduplication file.

As an example:

xtool.exe precomp -mzlib -c32mb -t75p --dedup=dupfile.bin data1.bin data1.unp

xtool.exe precomp -mzlib -c32mb -t75p --dedup=dupfile.bin data2.bin data2.unp

Assuming the files you are precompressing belong to the same package, their deduplication file can be shared, which means there will be only one deduplication file per package. (This is important if
you plan to use the program with Freearc)

XTool will determine which section deduplication file belonged to which input when you decode.


### Precompressor ❯ How_to_use_it

XTool





[XTool](#main-index)



# How to use it

Before you use XTool's precompressor, you should have prior knowledge of what compression algorithm is used on certain file types, document files typically are deflate compressed, such as docx and pdf
therefore a specific [codec](#precompressor-codecs) should be used on them (in their case that would be ZLib).

The command line of the precompressor looks like this:

xtool.exe precomp [parameters] input output (encoding)

xtool.exe decode [parameters] input output (decoding)

#### Input/Output

Input can be an existing file from disk, a standard input (stdin) or a direct link of a file to download and precompress (yes, this is supported).

Output can either can be an output file to the disk or a standard output (stdout).

Both stdin and stdout can be activated by not specifying input or output file or by simply writing "-"

**Example**

xtool.exe precomp -mzlib - -

xtool.exe decode - -

#### Parameters

|  |  |  |
| --- | --- | --- |
| Parameters | Explanation | Notes |
| -m# | Codecs to use for precompression (if more than one, separate their usage by using "+" between them)  example -mzlib+preflate | *See list of* [codecs](#precompressor-codecs) |
| -c# | Chunk size to use (kb,mb,gb can be used), default value is 16mb | Chunk size can be any value between 4MB and 2GB (higher value has chance of detecting large streams but at the consequence of high memory usage. |
| -t# | The number of threads to use, default is 50p of available CPU cores | You can specify the exact number of threads or a percentage. "p" denotes percentage. |
| -d# | Number of precompression depths to go through after finding a stream to find more streams within them, default is 0 | Sometimes streams contain additional streams, this is mostly seen in everyday files where already compressed files are compressed further by the user without knowing (zipping documents or pictures as an example) |
| -lm | This lowers memory usage of the precompression process at a cost of speed. | Under normal use, each thread gets their own chunk to scan but when low memory mode is used, only one chunk is used to scan streams from at a time. |
| --dbase | Make use of a stream database to speed up processing for common streams that are detected (repeated streams) | This feature is very useful as it gives speed boost but it is not enabled by default as there may be collision in the hashing that it uses. |
| --dedup=# | Enables stream deduplication and writes the information to a file output. (# denotes filename) | *More about deduplication* [here](#precompressor-deduplication) |
| --mem=# | Controls the amount of decoding memory to allocate for stream deduplication (once a certain threshold is reached, a virtual memory temp file is created), default is 75p of system memory. | You can specify an exact amount of memory it should use, 600mb as an example or as a percentage. "p" denotes percentage. |
| --diff=# | Control internal delta encoding threshold, default is 5p. | Streams that cannot be restored perfectly are processed using xdelta and if their difference crosses a certain threshold, they are discarded to avoid unwanted negative ratio. |

**Notes**

Mathematical expressions can be used in these parameters, for example when specifying decoding memory allocation, --mem=75p-600mb is 75% of the total system memory minus 600mb.

The same applies to specifying number of threads to use. -t100p-2 will use the total number of threads available minus 2. (If you wanted the user to have some system resource left to use the
computer)

|  |  |
| --- | --- |
| Command line examples (encoding) | What they do |
| xtool.exe precomp -mzlib+reflate -t100p-1 - - | use zlib and reflate to precompress input from stdin and write it to stdout while using total number of available threads minus one. |
| xtool.exe precomp -mpreflate -t3 https://www.7-zip.org/a/7za920.zip  7za920.zip.unp | use preflate, 3 threads, download and precompress file, write it to disk |
| xtool.exe precomp -mzlib+kraken:l4 -c256mb -lm - - | use zlib and kraken (try only level 4) to precompress data in 256mb chunks while using low memory, read from stdin and write to stdout |
| xtool.exe precomp -mzlib -d1 --dbase - - | use zlib to precompress data using depth 1 and use database to speed up the process. |
| xtool.exe precomp -mzstd --dbase --dedup=mydb.bin - - | use zstd to precompress data, use database to speed up the process and remove repetitions and store information to mydb.bin |

|  |  |
| --- | --- |
| Command line examples (decoding) | What they do |
| xtool.exe decode -t8 - data1.bin | decode data, use 8 threads, read from stdin and write to a filenamed data1.bin |
| xtool.exe decode -t100p-1 --dedup=mydb.bin - - | decode data, use 100 of CPU threads minus 1, use the database that was created when encoding to restore repeated streams. |


#### Precompressor ❯ Codecs ❯ Encryption

XTool





[XTool](#main-index)



# Encryption

At times, some files do contain encrypted data which prevent compression from taking place and given that not everyone is good at programming and can just create a dll on a whim however, if
you are in the field of data compression you should at least know how to handle configuration files (.ini) or better yet able to create database files.

Xtool offers the internal encryption codec which behaves like other internal codecs like zlib, oodle and etc but it can only be used by configuration/database codecs.

How does it work? Well configurations or database codecs typically find the location of streams and then the specified codec is used on those streams but what if the streams themselves are encrypted
and/or compressed, here is where you specify what encryption algorithm was used and then provide a key to be used to decrypt the located streams, xtool then passes the stream for decryption and if the depth
value was set higher than 0, then stream is then processed even further.

#### Algorithms available

aes, xor and rc4 (More can be added if requested)

The decryption key to use should be a stored binary file and the parameter of the codec should be the filename.

**Examples**

aes:mykey.bin

xor:fifakey.dat

**Notes**

The AES algorithm depends on the size of the key. To use AES-128 then the key should be 16-bytes in size, AES-224 should be 28 bytes and AES-256 should be 32 bytes.

The binary keys used for decryption are placed near the xtool executable and are only used when encoding and can be discarded when decoding.


#### Precompressor ❯ Codecs ❯ External_codecs

XTool





[XTool](#main-index)



# External codecs

External codecs are exactly as they sound like, they are codecs that were not internally built into xtool as these use other techniques which cannot generalised in the sense that the same codec
cannot be used for all forms of data. The external codec idea was introduced to ease the development of the main project as the coding required for some external codecs is complex and sometimes hinders
how the tool works as a whole and whenever there is a bug, it's a challenge to not only locate its origin but also how the bug needs to be fixed. The other reason external codecs exist is that it allows
the end-user to make their own codecs for whatever data they wish to add support for or perhaps process without the need of the xtool project being updated.

Here is a list of how you can make an external codec for it to be supported by xtool:

* [Configuration](#precompressor-codecs-externalcodecs-configuration)* [Database](#precompressor-codecs-externalcodecs-database)* [Library](#precompressor-codecs-externalcodecs-library)* Executable (coming soon)


#### Precompressor ❯ Codecs ❯ LZ4

XTool





[XTool](#main-index)



# LZ4

LZ4 is one of those libraries that are hard to make a universal scanner for streams and it's only included in xtool to be used by external codecs.

There are two codecs under this section that can be used. These codecs require liblz4.dll.

#### LZ4

aka lz4f (lz4 fast), this codec has no parameters and it is used as is.

#### LZ4HC

This is the high compression version of lz4.

|  |  |  |
| --- | --- | --- |
| Parameters | Explanation | Notes |
| l# | Levels to use for trial and error, by default it tries all | You can have add as many levels to try: l4:l5,... |

**Example**

lz4hc:l9,l10

**Note**

LZ4 does not have an internal scanner, it gets its streams from external codecs or when depth 1 or higher is used.


#### Precompressor ❯ Codecs ❯ LZO

XTool





[XTool](#main-index)



# LZO

Scanner of this codec isn't implemented therefore it's only included in xtool to be used by external codecs.

There is only one codec under this section that can be used for now. These codecs require lzo2.dll.

#### LZO1X

This is the high compression version of lz4.

|  |  |  |
| --- | --- | --- |
| Parameters | Explanation | Notes |
| l# | Levels to use for trial and error, by default it tries all | You can have add as many levels to try: l4:l5,... |
| v# | The LZO1X variant to use, the variants supported for now are: 999 | You need to be familiar with lzo to know what variants of the algorithm exist. |

**Example**

lzo1x:l9,v999

**Note**

LZ4O does not have an internal scanner for now but it gets its streams from external codecs or when depth 1 or higher is used.


#### Precompressor ❯ Codecs ❯ Oodle

XTool





[XTool](#main-index)



# Oodle

Oodle is a suite of compression algorithms used in newly released games, replacing the use of ZLib as it is slow when decoding.

Since this is a commercial compression library, the user must provide their own library to be able to use these codecs. The oodle library is usually shipped with the game files with the filename "oo2core\_\*\_win\*.dll"
or "oo2ext\_\*\_win\*.dll" ("oo2net\_\*\_win\*.dll" are not supported) .

#### Kraken

This uses oo2core\_\*\_win\*.dll, oo2ext\_\*\_win\*.dll or oodle\*.dll (whichever is present) to process the
streams using trial and error of level settings (9 combinations).

|  |  |  |
| --- | --- | --- |
| Parameters | Explanation | Notes |
| l# | Levels to use for trial and error, by default it tries all | You can have add as many levels to try: l4:l5,... |

**Example**

kraken:l7,l8

**Note**

It's wise to use the exact same library that came with your games files to precompress a certain input as each oodle library compresses files differently.

#### Mermaid

*See kraken information*

#### Selkie

*See kraken information*

#### Hydra

*See kraken information*

#### Leviathan

This codec cannot be used directly as there is currently no way of knowing information about scanned stream via trial and error therefore it can only be used by external codecs.

#### LZNA

*See leviathan information*


#### Precompressor ❯ Codecs ❯ ZLib

XTool





[XTool](#main-index)



# ZLib

The internal codec name is ZLib but it's actually deflate. Any data where the deflate algorithm was used, XTool should be able to pick it up.

The internal stream scanner by default scans for raw deflate streams, no headers (equivalent to -brute from precomp).

Deflate scanning is done by using (15) 32k window, however other window sizes are supported but these are searched via ZLib headers.

There are 4 codecs under this section that can be used on the streams that are found.

#### ZLib

This uses zlibwapi.dll or zlib1.dll (whichever is present) to process the streams using trial and error of
level and memory settings (9x9, 81 combinations).

|  |  |  |
| --- | --- | --- |
| Parameters | Explanation | Notes |
| l# | Levels ZLib should try (this includes memory setting), by default it tries all | You can have add as many levels to try: l68,l69,l98,l99... |
| w# | Window bits to use for deflate scanning, default is 15 | Read [ZLib documentation](https://zlib.net/manual.html) to learn more about window bits |

**Example**

zlib:l68,l69,w15

**Note**

The ZLib codec can be blended with reflate/preflate/grittibanzli (pick one), if all trials fail ZLib will use the other codec as backup. (e.g. -mzlib+reflate)

ZLib decodes faster than the other codecs because it doesn't produce any header information file and instead it just compresses the data directly so it is wise to combine it with the other
codecs.

#### Reflate

This uses hif2raw\_dll.dll and raw2hif\_dll.dll to process streams and requires a level setting.

The implementation of this tool is slower than it should due to the addition of stream verification to overcome internal bugs which result in failure to restore data with crc match (when
raw2hif is used, hif2raw is used at the same time to verify whether a stream can be restored perfectly).

|  |  |  |
| --- | --- | --- |
| Parameters | Explanation | Notes |
| l# | Reflate level to use (more than 1 level can be used) | Setting incorrect level increases header info file sizes resulting in bad results |
| w# | *See ZLib window bits* |  |

More than 1 level can be used however XTool will pick the one that gave good results (smaller header info file)

You can read more about level setting on the main thread [here](https://encode.su/threads/1399-reflate-a-new-universal-deflate-recompressor).

Example:

reflate:l6,l7,l8,l9

#### Preflate

This uses preflate\_dll.dll to process streams.

The library was modified and the multi-threading feature it offers was **removed** because it resulted in library not being thread-safe and XTool already
offers multi-threading of its own.

|  |  |  |
| --- | --- | --- |
| Parameters | Explanation | Notes |
| w# | *See ZLib window bits* |  |

#### GrittiBanzli

This uses grittibanzli\_dll.dll to process streams.

|  |  |  |
| --- | --- | --- |
| Parameters | Explanation | Notes |
| w# | *See ZLib window bits* |  |


#### Precompressor ❯ Codecs ❯ Zstd

XTool





[XTool](#main-index)



# ZStd

Even though lz4 and zstd are created by the same author, zstd is universal unlike lz4 due to several functions provided to help get stream information.

There is one codecs under this section that can be used. These codecs require libzstd.dll.

#### ZSTD

This is the high compression version of lz4.

|  |  |  |
| --- | --- | --- |
| Parameters | Explanation | Notes |
| l# | Levels to use for trial and error, by default it tries all | You can have add as many levels to try: l4:l5,... |

**Example**

zstd:l19,l22


##### Precompressor ❯ Codecs ❯ External_codecs ❯ Configuration

XTool





[XTool](#main-index)



# Configuration

Configuration based codec is simply an ini file that is filled with information which is then used to guide xtool in searching for particular streams.

They are stored in the extension **\*.ini** near the xtool executable and are only used when encoding and can be discarded when decoding.

These configuration files allow the user to make a simple stream search based on how they are stored in the data that is to be processed. Typically if the streams are stored in a specific repeating order that
form a certain pattern then configuration information can be given to xtool to make the search to find and process the streams in question.

Now what does it mean, "repeating order" and "pattern"? Well for starters, archives or data in general often contains file headers/signature or file format information which hold encoded information
that helps identify what kind of file or data is stored on the creation of a certain file/archive. A simple example would be \*.jpg image. This does have its own unique file signature to help other
programs or users to identify it as an image apart from looking at extension of the file.

![](Precompressor/Codecs/External_codecs/Configuration/1.png)

From a binary standpoint, to identify jpg from a random data you just need to look for this signature (magic number) **FF D8 FF** then you realize that this could perhaps be an image.
PNG image has its own file signature. So going back to "repeating order" and "pattern", if you archived a certain number of jpg images in a zip archive. If you were to search for the **FF D8 FF** signature,
you'll notice a trend of repeating jpg signatures that sort of form a pattern, one after the other. This trend that you notice can be information which you can write in a configuration which would
then make xtool to search and then you give instructions on how to process such data.

---

#### See also

[How to write a configuration file](#precompressor-codecs-externalcodecs-configuration-howtowriteaconfigurationfile)

[Examples](#precompressor-codecs-externalcodecs-configuration-examples)


##### Precompressor ❯ Codecs ❯ External_codecs ❯ Database

XTool





[XTool](#main-index)



# Database

Databases are **generated** files that store a collection of stream information. At times creating library or a configuration based codec isn't sufficient to capture streams due
to stream information being unavailable or inaccessible therefore database "based" codecs are needed as they will be the ones that will contain the necessary information.

They are stored in the extension **\*.xtl** near the xtool executable and are only used when encoding and can be discarded when decoding.

You can use tools from this [forum thread](https://fileforums.com/showthread.php?t=104109) to automatically create database files provided you know what method was used on the
files but if you plan on making your own database file from scratch then look at the format from [here](#precompressor-codecs-externalcodecs-database-format).


##### Precompressor ❯ Codecs ❯ External_codecs ❯ Library

XTool





[XTool](#main-index)



# Library

Library based codecs are dynamic-link library files that export functions to be used by xtool, they are a more advanced and direct way of adding support for a codec that isn't supported internally
by the program and they require some programming knowledge.

The reason they are advanced is because xtool gives you direct access to the data which you can use to manually search streams for streams or obtain them from other external codecs and then
process them.

As mentioned before, external codecs can give streams to the internal codecs for them to be processed the same is true for library based codecs as they too can accept streams given by configuration/database
codecs as an example, you can create lzx library and then make configurations or database codecs give the streams to the library. The library itself doesn't really need to be able to scan for streams for
it to be useful.

There are a total of 7 functions that need to be exported by the library in order for it to be recognized by xtool and these functions are listed below as

* [PrecompInit](#precompressor-codecs-externalcodecs-library-precompinit) * [PrecompFree](#precompressor-codecs-externalcodecs-library-precompfree) * [PrecompCodec](#precompressor-codecs-externalcodecs-library-precompcodec) * [PrecompScan1](#precompressor-codecs-externalcodecs-library-precompscan1) * [PrecompScan2](#precompressor-codecs-externalcodecs-library-precompscan2) * [PrecompProcess](#precompressor-codecs-externalcodecs-library-precompprocess) * [PrecompRestore](#precompressor-codecs-externalcodecs-library-precomprestore)


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Configuration ❯ Examples

XTool





[XTool](#main-index)



# Examples

#### Cyberpunk 2077

This game is compressed using oodle because it shipped with **oo2ext\_7\_win64.dll** so we now have to look for oodle headers. Kraken (8C 06), Mermaid (8C 0A), Leviathan (8C 0C), LZNA (8C
05)

![](Precompressor/Codecs/External_codecs/Configuration/Examples/6.png)

So then we look for these oodle headers and we find that there is one within the data

![](Precompressor/Codecs/External_codecs/Configuration/Examples/7.png)

and another one was found

![](Precompressor/Codecs/External_codecs/Configuration/Examples/9.png)

So we can confirm that the data was compressed using **Kraken** (8C 06) but what else can we find from the hex data.

We also notice that there is the text "KARK" **4B 41 52 4B** that keeps showing up before 8C 06 so we can safely say that this is the signature.

First occurrence `4B 41 52 4B 4B 01 00 00 8C 06`

Second occurrence `4B 41 52 4B 3C 01 00 00 8C 06`

So we have found our **repeating pattern** because **4B 41 52 4B** always appears then after 4 bytes our Kraken header **8C 06** appears.

Our structure then is Signature(4),Unknown(4),OodleHdr(2),Stream.

We still have that unknown bytes that keeps changing  between the Signature and the Kraken header, what could it be? Maybe the size of the stream? (If you're familiar with oodle, you should know
that the decompressed size is important else the stream cannot be decompressed), so we can assume that this is the DecompressedSize

We can then rename our Unknown variable to DSize which stands for DecompressedSize.

Since 8C 06 is "part" of the stream, our stream offset should be -2 so that we include the
oodle header as part of the entire stream.

For the streams to be valid we need to make sure that they all contain the Oodle header, 8C 06 (0x068C) as we will know that these are kraken streams so that's our "condition"

The configuration is now complete and we fill it in such a manner.

```
[Stream1]  
Name=kraken  
Codec=kraken  
BigEndian=0  
Signature=0x4B52414B  
Structure=Signature(4),DSize(4),OodleHdr(2),Stream  
StreamOffset=-2  
CompressedSize=0  
DecompressedSize=DSize  
Condition1=OodleHdr = 0x068C
```

**Notes**

The name of the codec is kraken because the streams we are trying to detect are kraken streams.

The codec these streams are compressed with is Kraken, if we also know the level that was used we can specify it as Kraken:l5 but for now as we don't know much about the streams we write Kraken making
xtool to try every level until it finds the correct one.

If the game was compressed using Kraken and zstd.

We need to make Stream2 for zstd streams and it should look like this

```
[Stream2]  
Name=zstd  
Codec=zstd  
BigEndian=0  
Signature=0x4B52414B  
Structure=Signature(4),DSize(4),ZstdHdr(4),Stream  
StreamOffset=-4  
CompressedSize=0  
DecompressedSize=DSize  
Condition1=ZstdHdr = 0xFD2FB528
```

Note, zstd header is **28 B5 2F FD** and it consumes 4 bytes and since it's part of the stream, our stream offset is -4.

If we then wanted to use the configuration to **only** process zstd streams, we specify -mcp2077:zstd (provided config is saved as cp2077.ini) since that's the **name** of
the streams as specified in the configuration and all kraken streams will be ignored.


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Configuration ❯ How_to_write_a_configuration_file

XTool





[XTool](#main-index)



# How to write a configuration file

 Writing a configuration file is simple as long as you're familiar with reading binary data and how the program such as [HxD](https://mh-nexus.de/en/hxd/) is
used.

The structure of a configuration file looks like this

![](Precompressor/Codecs/External_codecs/Configuration/How_to_write_a_configuration_file/3.png)

The section name of ini are in the format Stream#, where # denotes the index of stream variation that needs to be searched in the data to be
processed.

As an example, if you are creating an image processing codec that handles jpg, png and gif images, obviously you cannot put all information of three different image types in the same section so, you
can add information of jpg in Stream1 section, information of png in Stream2 section and so forth so that when the configuration is called by xtool, it searches for all images types at the same time.

In every section, there are value names that need to be filled in and the table below briefly explains what each value name does and how it should be filled in.

|  |  |  |
| --- | --- | --- |
| Value name | Explanation | Notes |
| Name | The name of streams that belong to the section. | As mentioned before, you can have several stream searches jpg, png and gif. If the data for whatever reason contains more than one sets of data, you can give it a name: lz4, lz4crypt... |
| Codec | The codec to use for processing this stream. | You can choose any of the internal or external codecs to use: lz4, zstd, leviathan... |
| BigEndian | Denotes whether the file header is stored in big or little endian. | *See* [*here*](https://en.wikipedia.org/wiki/Endianness)*.* 0 = little, 1 = big. |
| Signature | The unique sequence or value used to identify a file's header structure (magic number). | As an example made above of jpg having **FF D8 FF,** the value here would be 0xFFD8FF ($ can also be used as hex prefix or decimal value 16767231) |
| Structure | The file header structure. This contains information about the stored stream. | See *stream structure*below |
| StreamOffset | The position at which the stream is out of line. | See *stream structure* below |
| CompressedSize | The size of the stream when stored. | Variables can be used.  *Learn more about variables below* |
| DecompressedSize | The size of the stream after its processed. | Variables can be used.  *Learn more about variables below* |
| Condition# | A check performed to confirm whether something is true or false, if the condition is true then the stream is accepted but if it's false then the stream is discarded. | Conditions are used to reduce or eliminate false positives detected during scans. # denotes condition index and several conditions can be specified to make sure the scanned stream is correctly detected.  *All conditions must be true for the stream to be accepted.* |

**Stream structure**

File headers/structures are *often* stored in a fixed number of bytes and within these bytes, you obtain information about the stream that follows.

Now, the jpg file header is complex but for beginners so we shall use wav audio file as an example which looks roughly like this

![](Precompressor/Codecs/External_codecs/Configuration/How_to_write_a_configuration_file/4.png)

The detailed file header of wav audio can be found [here](https://docs.fileformat.com/audio/wav/), now then

![](Precompressor/Codecs/External_codecs/Configuration/How_to_write_a_configuration_file/5.png)

From this information, we can safely say that the signature of a wav audio file is "RIFF" (in hex **52 49 46 46**) because they all seem to begin in this manner. Note that this consumes 4
bytes in total.

Then we are told that the file size is what followers after RIFF, from the example above (**44 80 A3 02**, which is 44269636), this is also consumes 4
bytes. The same file size can also be our CompressedSize.

We also then told that after the file size, the constant WAVE also appears which is (**57 41 56 45**), also 4 bytes.

So now we know 3 variables from the wav audio header structure. We know the signature, also the size of the wav file and constant that follows "WAVE". In total **12 bytes** so how do
we write this information under the Stream section?

```
Signature=0x46464952  
Structure=Signature(4),FileSize(4),FileType(4),Stream
```

Simply put, the **signature** is 4 bytes, the file size is also 4 bytes same as the file type and what follows after is the **stream**.

Note, the names used in the structure i.e. FileSize, FileType are variables that you as user declared and they can be called whatever you prefer, even FS as file size for short, just as long as
you as a user know what the variable contains. **Signature** is also a variable but this cannot be renamed to whatever the user prefers
as this variable is tied to what the xtool program is expecting, same goes for the variable **Stream**.

Every variable has a size and the size is specified after the variable name enclosed by brackets. e.g. MyVariable(8) and within the file signature, every variable is separated by a comma. e.g.
Signature(4),MyVar1(6),MyVar2(2)...

**Stream** is the only variable that does not contain the size of variable enclosed by brackets as this denotes the starting point
of the actual data to be processed therefore the entire structure is written as above.

Now to complete configuration, we have CompressedSize and DecompressedSize as blank. But if you recall, from the headers, we have the FileSize as a variable so why not use this to fill in CompressedSize.

```
CompressedSize=FileSize
```

We are told that file size is actually overall size minus 8 bytes so to specify the correct size. We then need to add 8 bytes to the FileSize value to get the correct CompressedSize therefore this can
now be written as

```
CompressedSize=FileSize + 8
```

Since we do not know the DecompressedSize, we can specify 0.

As for StreamOffset, since the structure we specified `Signature(4),FileSize(4),FileType(4)` consumes 12 bytes and it's part of the entire stream, it means
the stream actually starts 12 bytes before the start of the signature therefore our StreamOffset is -12.

**Conditions**

As explained before, conditions are checks to see whether certain variables are true or not.

To make sure that the streams we are searching for are actual wave audio streams we need to make use of the file header variables to help substantiate that fact.

From the structure, we have used Signature to help us find the stream. The FileSize was used to help us find the CompressedSize of the stream but what else can we do to make sure this stream is valid?
We still have FileType which is said to be always equal to "WAVE" **57 41 56 45** so from this, we can start by making our first condition that FileType should be always equals to 0x45564157.

```
Condition1=FileType = 0x45564157
```

 If for whatever reason, we want to target streams that are *equal or larger* than 4096 bytes we can make our second condition

```
Condition2=FileSize >= 4096
```

Our configuration is now complete and it should look like this

```
[Stream1]  
Name=wav  
Codec=wavpack  
BigEndian=0  
Signature=0x46464952  
Structure=Signature(4),FileSize(4),FileType(4),Stream  
StreamOffset=-12  
CompressedSize=FileSize + 8  
DecompressedSize=0  
Condition1=FileType = 0x45564157  
Condition2=FileSize >= 4096
```

The codec used is wavpack (it could also be tak), but you might say these codecs do not exist and yes they don't however. If you have a library or executable of wavpack, you can place it near and register
it to xtool as another external codec then all the streams found via the configuration file are then processed by wavpack.

**Notes**

Algebraic expressions/equations, trigonometric functions, logarithmic function, and binary operators (both Pascal and C++ syntax) can be used when writing a configuration file.

e.g. CompressedSize=(1 shl 16) + FileSize16


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Database ❯ Format

XTool





[XTool](#main-index)



# Format

If for whatever reason you wanted to make your own database without using the provided tools, this is the database format.

#### Header

|  |  |  |
| --- | --- | --- |
|  | Size | Description |
| Magic number | 4-bytes | 0x42445458 *"XTDB"* |
| File information | n-bytes | *See file information* (you can have more than one) |

A single database file can have multiple file informations stored one after the other, which is ideal for adding information for multiple files that come from the same application.

#### File information

|  |  |  |
| --- | --- | --- |
|  | Size | Description |
| Identifier | 8-bytes | The first 8 bytes of file |
| Hash length | 4-bytes | The length of the hash for the first n-bytes |
| Hash digest | 16-bytes | The hash value. (MD5) |
| Codec length | 4-bytes | The length of the codec |
| Codec string | n-bytes | The string value of the codec (ANSI/UTF-8) |
| Stream count | 4-bytes | The number of streams the file contains |
| Stream information | Count\*16-bytes | *See stream information* |

The hash algorithm used is MD5.

#### Stream information

|  |  |  |
| --- | --- | --- |
|  | Size | Description |
| Offset | 8-bytes | The position of the stream |
| Original size | 4-bytes | The compressed size |
| Unpacked size | 4-bytes | The decompressed size |


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Library ❯ Add

XTool





[XTool](#main-index)



# Add

Add is a callback function used to notify xtool of a possible stream.

`type  
 TPrecompAdd = procedure(Instance: Integer; Info: PStrInfo1; Codec: PChar; DepthInfo: PDepthInfo)cdecl;`

Instance is the thread that called the function.

Info is the information of the stream.

DepthInfo is optional, can be left nil or NULL especially if depth info is unknown. [See here](#precompressor-codecs-externalcodecs-library-types).


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Library ❯ Funcs

XTool





[XTool](#main-index)



# Funcs

Funcs is a callback function that provides a list of functions that may be of use when making your library.

`type  
PPrecompFuncs = ^TPrecompFuncs;  
TPrecompFuncs = record  
 Allocator: function(Index: Integer; Size: Integer): Pointer cdecl;  
 GetCodec: function(Cmd:
PChar; Index: Integer; Param: Boolean): TPrecompCmd cdecl;  
 GetParam: function(Cmd: PChar; Index: Integer; Param: PChar): TPrecompCmd cdecl;  
 GetDepthInfo: function(Index: Integer): TDepthInfo
cdecl;  
 Compress: function(Codec: PChar; InBuff: Pointer; InSize: Integer;OutBuff: Pointer; OutSize: Integer; DictBuff: Pointer; DictSize: Integer): Integer cdecl;  
 Decompress: function(Codec:
PChar; InBuff: Pointer; InSize: Integer; OutBuff: Pointer; OutSize: Integer; DictBuff: Pointer; DictSize: Integer): Integer cdecl;  
 Encrypt: function(Codec: PChar; InBuff: Pointer; InSize: Integer;
KeyBuff: Pointer; KeySize: Integer): Boolean cdecl;  
 Decrypt: function(Codec: PChar; InBuff: Pointer; InSize: Integer; KeyBuff: Pointer; KeySize: Integer): Boolean cdecl;  
 Hash: function(Codec:
PChar; InBuff: Pointer; InSize: Integer; HashBuff: Pointer; HashSize: Integer): Boolean cdecl;  
 EncodePatch: function(OldBuff: Pointer; OldSize: Integer; NewBuff: Pointer; NewSize: Integer; PatchBuff:
Pointer; PatchSize: Integer): Integer cdecl;  
 DecodePatch: function(PatchBuff: Pointer; PatchSize: Integer; OldBuff: Pointer; OldSize: Integer; NewBuff: Pointer; NewSize: Integer): Integer cdecl;  
 AddResource:
function(FileName: PChar): Integer cdecl;  
 GetResource: function(ID: Integer; Data: Pointer; Size: PInteger): Boolean cdecl;  
 SearchBinary: function(SrcMem: Pointer; SrcPos, SrcSize: NativeInt;
SearchMem: Pointer; SearchSize: NativeInt; ResultPos: PNativeInt): Boolean cdecl;  
 SwapBinary: procedure(Source, Dest: Pointer; Size: NativeInt)cdecl;  
 Swap16: function(Value: ShortInt):
ShortInt cdecl;  
 Swap32: function(Value: Integer): Integer cdecl;  
 Swap64: function(Value: Int64): Int64 cdecl;  
 FileOpen: function(FileName: PChar; Create: Boolean): THandle cdecl;  
 FileClose:
procedure(Handle: THandle)cdecl;  
 FileSeek: function(Handle: THandle; Offset: Int64; Origin: Integer): Int64 cdecl;  
 FileSize: function(Handle: THandle): Int64 cdecl;  
 FileRead:
function(Handle: THandle; Buffer: Pointer; Count: Integer): Integer cdecl;  
 FileWrite: function(Handle: THandle; Buffer: Pointer; Count: Integer): Integer cdecl;  
 IniRead: function(Section,
Key, Default, FileName: PChar): TPrecompCmd cdecl;  
 IniWrite: procedure(Section, Key, Value, FileName: PChar)cdecl;  
 Reserved: array [0 .. (PRECOMP_FCOUNT - 1) - 26] of
Pointer;  
end;`

**Functions** 

Allocator - Global memory allocator, useful for reducing memory usage of xtool

GetCodec - Parses command string, returns codec based on index.

GetCodec - Parses command string, returns parameter value based on index and parameter prefix.

GetDepthInfo - Returns depth stream information.

Compress - Data compression function. (codec = 'zlib', 'lz4', 'lz4hc', 'lzo1c', 'lzo1x', 'lzo2a', 'zstd', 'lzna', 'kraken', 'mermaid', 'selkie', 'hydra', 'leviathan')

Decompress - Data decompression function. (codec = 'zlib', 'lz4', 'lz4hc', 'lzo1c', 'lzo1x', 'lzo2a', 'zstd', 'lzna', 'kraken', 'mermaid', 'selkie', 'hydra', 'leviathan')

Encrypt - Data encryption function. (codec = 'xor', 'aes', 'rc4')

Decrypt - Data decryption function. (codec = 'xor', 'aes', 'rc4')

Hash - Computes hash on a given data. (codec = 'crc32', 'adler32', 'crc64', 'md5', 'sha1')

EncodePatch - Uses xdelta to create a diff between two sets of data.

DecodePatch - Uses xdelta to create a patch.

AddResource - Adds a file to be stored within xtool which can be called from memory.

GetResource - Retrives memory of a file that was added.

SearchBinary - Performs a binary search of a specified memory buffer and returns location.

SwapBinary - Reverses the binary byte order.

Swap16, Swap32, Swap64 - Changes Endianess.

FileOpen - Opens a file.

FileClose - Closes a file.

FileSeek - Sets the position of the file.

FileRead - Read data from the current position.

FileWrite - Writes data at the current position.

IniRead - Reads a string from a ini file.

IniWrite - Writes a string to a ini file.

Reserved - functions that can be added in future.


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Library ❯ Output

XTool





[XTool](#main-index)



# Output

Output is a callback function used for writing decoded output data of a stream.

`type  
 TPrecompOutput = procedure(Instance: Integer; const Buffer: Pointer;Size: Integer)cdecl;`

Instance is the thread that called the function.

Buffer is the memory data to be written.

Size of the memory buffer.


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Library ❯ PrecompCodec

XTool





[XTool](#main-index)



# PrecompCodec

#### Function prototype

`function PrecompCodec(Index: Integer): PChar cdecl;`

#### Description

Returns a name of the codec given its index in the list the library provides.

#### Parameters

**Index**

The index of the codec

#### Return value

Give the name of the codec based on index from codec list, return **nil** or **NULL** to indicate the end of list.


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Library ❯ PrecompFree

XTool





[XTool](#main-index)



# PrecompFree

#### Function prototype

`procedure PrecompFree(Funcs: PPrecompFuncs) cdecl;`

#### Description

Deinitialises the library

#### Parameters

**Funcs**

[See here](#precompressor-codecs-externalcodecs-library-funcs)


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Library ❯ PrecompInit

XTool





[XTool](#main-index)



# PrecompInit

#### Function prototype

`function PrecompInit(Command: PChar; Count: Integer; Funcs: PPrecompFuncs): Boolean cdecl;`

#### Description

Initialises the library

#### Parameters

**Command**

The codec command line specified by user.

**Count**

The number of threads specified by user.

**Funcs**

[See here](#precompressor-codecs-externalcodecs-library-funcs)

#### Return value

State whether library initialised successfully.


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Library ❯ PrecompProcess

XTool





[XTool](#main-index)



# PrecompProcess

#### Function prototype

`function PrecompProcess(Instance: Integer; OldInput, NewInput: Pointer; StreamInfo: PStrInfo2; Output: TPrecompOutput; Funcs: PPrecompFuncs): Boolean cdecl;`

#### Description

Offers a last opportunity to decode a stream.

#### Parameters

**Instance**

The thread that is calling the function.

**OldInput**

The memory buffer of original data.

**NewInput**

The memory buffer of the decoded data.

**StreamInfo**

The information of the stream.

See here

**Output**

The function to use for writing additional output of the stream.

Writing here is optional and should be used if you want to store additional data to use when decoding.

[See here](#precompressor-codecs-externalcodecs-library-output)

**Funcs**

[See here](#precompressor-codecs-externalcodecs-library-funcs)

#### Return value

State whether the stream was processed successfully.


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Library ❯ PrecompRestore

XTool





[XTool](#main-index)



# PrecompRestore

#### Function prototype

`function PrecompRestore(Instance: Integer; Input, InputExt: Pointer; StreamInfo: TStrInfo3;
Output: TPrecompOutput; Funcs: PPrecompFuncs): Boolean cdecl;`

#### Description

Offers a last opportunity to decode a stream.

#### Parameters

**Instance**

The thread that is calling the function.

**Input**

The memory buffer of decoded data.

**InputExt**

The memory buffer of the data you written in PrecompProcess' Output function.

**StreamInfo**

The information of the stream.

See here

**Output**

The function to use for writing the original stream data.

[See here](#precompressor-codecs-externalcodecs-library-output)

**Funcs**

[See here](#precompressor-codecs-externalcodecs-library-funcs)

#### Return value

State whether the stream was restored successfully.


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Library ❯ PrecompScan1

XTool





[XTool](#main-index)



# PrecompScan1

#### Function prototype

`procedure PrecompScan1(Instance: Integer; Input: PByte; Size, SizeEx: Int64; Output: TPrecompOutput; Add: TPrecompAdd; Funcs: PPrecompFuncs) cdecl;`

#### Description

Scans a provided memory buffer for streams.

#### Parameters

**Instance**

The thread that is calling the function.

**Input**

The memory buffer to scan from.

**Size**

The scanning memory range (do not exceed when scanning)

**SizeEx**

The actual size of the memory buffer. The starting point of the streams shall not start after the scanning range *if it is to be decoded immediately* (if you
will be using **output** function).

**Output**

The function to use for writing the decoded output of the stream.

Writing to output is optional, but if you choose to add the stream then.

[See here](#precompressor-codecs-externalcodecs-library-output)

**Add**

The function to use for confirming that the stream could be valid while specifying additional information about it.

[See here](#precompressor-codecs-externalcodecs-library-add)

**Funcs**

[See here](#precompressor-codecs-externalcodecs-library-funcs)


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Library ❯ PrecompScan2

XTool





[XTool](#main-index)



# PrecompScan2

#### Function prototype

`function PrecompScan2(Instance: Integer; Input: Pointer; Size: Int64; StreamInfo: PStrInfo2; Output: TPrecompOutput; Funcs: PPrecompFuncs) : Boolean cdecl;`

#### Description

Offers a last opportunity to decode a stream.

#### Parameters

**Instance**

The thread that is calling the function.

**Input**

The memory buffer of the future stream.

**Size**

The size of the buffer.

**StreamInfo**

The information of the stream.

See here

**Output**

The function to use for writing the decoded output of the stream.

Writing to output is no longer optional and now necessary.

[See here](#precompressor-codecs-externalcodecs-library-output)

**Funcs**

[See here](#precompressor-codecs-externalcodecs-library-funcs)

#### Return value

State whether the stream is valid or not.


###### Precompressor ❯ Codecs ❯ External_codecs ❯ Library ❯ Types

XTool





[XTool](#main-index)



# Types

`type  
PPrecompCmd = ^TPrecompCmd;

TPrecompCmd = array [0 .. 255] of Char;

PDepthInfo = ^TDepthInfo;

TDepthInfo = packed record  
 Codec: array [0 .. 59] of Char;  
 OldSize: Integer;  
 NewSize: Integer;  
end;

PStrInfo1 = ^TStrInfo1;

TStrInfo1 = packed record  
 Position: Int64;  
 OldSize, NewSize: Integer;  
 Resource: Integer;  
 Option: Word;  
end;

PStrInfo2 = ^TStrInfo2;

TStrInfo2 = packed record  
 OldSize, NewSize: Integer;  
 Resource: Integer;  
 Option: Word;  
end;

PStrInfo3 = ^TStrInfo3;

TStrInfo3 = packed record  
 OldSize, NewSize, ExtSize: Integer;  
 Resource: Integer;  
 Option: Word;  
end;`

**TPrecompCmd** is a 256-lengthed unicode string.

**TDepthInfo** is used when information of a stream within a stream is known.

**Resource** is the index of the resource. See AddResource/GetResource [here](#precompressor-codecs-externalcodecs-library-funcs).

**Option** stores a value that can be retrieved whenever. (Can be used to store restoration information, like compression level etc)

**ExtSize** is the size of **InputExt** buffer used in [PrecompRestore](#precompressor-codecs-externalcodecs-library-precomprestore).