//! Original misaki pronunciation data (Apache-2.0); tagger from misaki-rs (MIT).
use crate::engine::{DownloadFile, DownloadSpec};
pub const FILES: &[DownloadFile] = &[
    DownloadFile {
        name: "us_gold.json",
        spec: DownloadSpec {
            url: "https://raw.githubusercontent.com/hexgrad/misaki/e820629b96334db28227df37f280e4836d46fadb/misaki/data/us_gold.json",
            size: 3000469,
            sha256: "dc414872a49a28ae6c141463d502fd945f3b2fde040484fdc47d00cc4612686f",
        },
    },
    DownloadFile {
        name: "us_silver.json",
        spec: DownloadSpec {
            url: "https://raw.githubusercontent.com/hexgrad/misaki/e820629b96334db28227df37f280e4836d46fadb/misaki/data/us_silver.json",
            size: 3099517,
            sha256: "de8f67be911bb6c659187b4a65fd966b6a30e56350e0f790d763210b053ac475",
        },
    },
    DownloadFile {
        name: "gb_gold.json",
        spec: DownloadSpec {
            url: "https://raw.githubusercontent.com/hexgrad/misaki/e820629b96334db28227df37f280e4836d46fadb/misaki/data/gb_gold.json",
            size: 2838552,
            sha256: "29e62f4b60261c88f7f3c2c7811ca3825978948090b72d2b27d565b729282f71",
        },
    },
    DownloadFile {
        name: "gb_silver.json",
        spec: DownloadSpec {
            url: "https://raw.githubusercontent.com/hexgrad/misaki/e820629b96334db28227df37f280e4836d46fadb/misaki/data/gb_silver.json",
            size: 3663898,
            sha256: "48131e2d92ccc41655f4543e87e0f938e71463eb5a54be7f0693bb712ebb6bce",
        },
    },
    DownloadFile {
        name: "tagger/weights.json",
        spec: DownloadSpec {
            url: "https://raw.githubusercontent.com/MicheleYin/misaki-rs/38bf1a534cce5f6864fb2df595447ca6f6891e74/src/resources/tagger/weights.json",
            size: 5677744,
            sha256: "789e8e35f6fac656d9b7eccea29ba4e8a137cbb5b525dd31f39e4291a9e80832",
        },
    },
    DownloadFile {
        name: "tagger/classes.txt",
        spec: DownloadSpec {
            url: "https://raw.githubusercontent.com/MicheleYin/misaki-rs/38bf1a534cce5f6864fb2df595447ca6f6891e74/src/resources/tagger/classes.txt",
            size: 149,
            sha256: "7812cb7945022cb348bc6670b3eb7c76cdd50920715e86681337adfbf9c85253",
        },
    },
    DownloadFile {
        name: "tagger/tags.json",
        spec: DownloadSpec {
            url: "https://raw.githubusercontent.com/MicheleYin/misaki-rs/38bf1a534cce5f6864fb2df595447ca6f6891e74/src/resources/tagger/tags.json",
            size: 25788,
            sha256: "4c713731cb06727736962bc8cd94ad919217a040e76691059af99bc0c2c9c246",
        },
    },
];
