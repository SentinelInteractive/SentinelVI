use sentinel_toc::{FullToc, Paths, SlotFilter, SlotSearch};

fn fixture() -> (std::path::PathBuf, FullToc) {
    let db = std::env::temp_dir().join(format!(
        "sentinel-toc-{}-{:?}.db",
        std::process::id(),
        std::thread::current().id()
    ));
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TABLE roots(root TEXT PRIMARY KEY, catalog_key TEXT, headers INTEGER,
          entries INTEGER, physical_bytes INTEGER);
        CREATE TABLE headers(root TEXT, header_offset INTEGER, signature_status TEXT,
          PRIMARY KEY(root, header_offset)) WITHOUT ROWID;
        CREATE TABLE slots(root TEXT, archive_offset INTEGER, slot INTEGER,
          data_offset_absolute INTEGER, data_offset_relative INTEGER, stored_bytes INTEGER,
          logical_bytes INTEGER, kind TEXT, hash32 INTEGER, hash64 INTEGER, path TEXT,
          name_origin TEXT, signature_status TEXT, header_offset INTEGER,
          PRIMARY KEY(root, archive_offset, slot), CHECK(archive_offset = header_offset),
          CHECK ((path IS NULL) = (name_origin = 'UnresolvedHash')));
        CREATE INDEX slots_root ON slots(root);
        CREATE INDEX slots_hash32 ON slots(hash32) WHERE hash32 IS NOT NULL;
        CREATE INDEX slots_hash64 ON slots(hash64) WHERE hash64 IS NOT NULL;
        CREATE INDEX slots_path ON slots(path) WHERE path IS NOT NULL;
        INSERT INTO roots VALUES ('base', 'a', 2, 3, 1000), ('other', 'b', 1, 1, 1000),
          ('paged', 'c', 1, 205, 1000);
        INSERT INTO headers VALUES ('base', 64, 'Unknown'), ('base', 128, 'Unknown'),
          ('other', 64, 'Unknown'), ('paged', 64, 'Unknown');
        INSERT INTO slots VALUES
          ('base', 64, 1, 80, 16, 8, 12, 'binary', 0xfedcba98, 0x0123456789abcdef,
           'folder/a_b.ext', 'DictionaryMatch', 'Unknown', 64),
          ('base', 64, 2, 96, 32, 6, 6, 'binary', NULL, 0xfedcba9876543210,
           'not/a/dictionary/name', 'Original', 'Unknown', 64),
          ('base', 128, 1, 128, 0, 0, 0, 'directory', NULL, NULL,
           NULL, 'UnresolvedHash', 'Unknown', 128),
          ('other', 64, 3, 80, 16, 4, 4, 'binary', NULL, NULL,
           'folder/acb.ext', 'DictionaryMatch', 'Unknown', 64);
        WITH RECURSIVE ids(n) AS (SELECT 100 UNION ALL SELECT n+1 FROM ids WHERE n < 304)
          INSERT INTO slots SELECT 'paged', 64, n, 64, 0, 0, 0, 'binary', NULL,
            NULL, NULL, 'UnresolvedHash', 'Unknown', 64 FROM ids;",
    )
    .unwrap();
    drop(conn);
    let toc = FullToc::open(&db).unwrap();
    (db, toc)
}

#[test]
fn real_index_pages_metadata_and_search_without_assets() {
    let (db, toc) = fixture();
    assert_eq!(toc.roots().unwrap()[0].slot_count, 3);
    let headers = toc.headers("base", None, 1).unwrap();
    assert_eq!(headers.rows[0].header_offset, 64);
    assert_eq!(headers.rows[0].slot_count, 2);
    assert_eq!(
        toc.headers("base", headers.next, 1).unwrap().rows[0].header_offset,
        128
    );
    let filter = SlotFilter {
        root: Some("base"),
        header_offset: Some(64),
        search: None,
    };
    let first = toc.slots(&filter, None, 1).unwrap();
    let second = toc.slots(&filter, first.next, 1).unwrap();
    assert_eq!(first.rows[0].display_name, "folder/a_b.ext");
    assert_eq!(first.rows[0].name_origin, "DictionaryMatch");
    assert_eq!(second.rows[0].display_name, "Hash fedcba9876543210");
    assert_eq!(second.rows[0].path, None);
    assert_eq!(second.next, None);
    assert_eq!(toc.slot(3).unwrap().unwrap().display_name, "Slot 1");
    assert_eq!(toc.slot(1).unwrap().unwrap().data_offset_relative, 16);
    assert_eq!(
        toc.slot(1).unwrap().unwrap().hash32.as_deref(),
        Some("fedcba98")
    );
    assert!(toc.slot(999).unwrap().is_none());
    for (search, ids) in [
        (SlotSearch::Path("A_B"), vec![1]),
        (SlotSearch::Path("dictionary"), vec![]),
        (SlotSearch::Hash32(0xfedcba98), vec![1]),
        (SlotSearch::Hash64(0xfedcba9876543210), vec![2]),
        (SlotSearch::Root("other"), vec![4]),
        (SlotSearch::Slot(1), vec![1, 3]),
    ] {
        let page = toc
            .slots(
                &SlotFilter {
                    search: Some(search),
                    ..SlotFilter::default()
                },
                None,
                1000,
            )
            .unwrap();
        assert_eq!(page.rows.into_iter().map(|s| s.id).collect::<Vec<_>>(), ids);
    }
    let filter = SlotFilter {
        root: Some("paged"),
        ..SlotFilter::default()
    };
    let first = toc.slots(&filter, None, u32::MAX).unwrap();
    assert_eq!(first.rows.len(), 200);
    let last = toc.slots(&filter, first.next, u32::MAX).unwrap();
    assert_eq!(last.rows.len(), 5);
    assert_eq!(last.rows[0].slot, 300);
    assert_eq!(last.next, None);
    drop(toc);
    std::fs::remove_file(db).unwrap();
}

#[test]
fn only_an_absent_primary_index_selects_fallback() {
    let root = std::env::temp_dir().join(format!("sentinel-library-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let paths = Paths {
        root: root.clone(),
        game: root.clone(),
    };
    assert!(FullToc::open_if_present(&paths).unwrap().is_none());
    std::fs::write(root.join("vi_toc_full.db"), "not sqlite").unwrap();
    assert!(FullToc::open_if_present(&paths).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
