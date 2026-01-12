import functools
import itertools
import logging
import os.path

# import operator
from numbers import Number
from typing import Any, Callable, Dict, Iterable, List, Set, Tuple, Union, cast

import openpyxl
import pandas as pd
from openpyxl import Workbook
from openpyxl.cell.cell import Cell
from openpyxl.utils import get_column_letter
from openpyxl.worksheet.cell_range import MultiCellRange
from openpyxl.worksheet.datavalidation import DataValidation
from openpyxl.worksheet.worksheet import Worksheet
from rapidfuzz import fuzz

from .utils import (
    _match_value_to_rule,
    _read_cell_style,
    load_charts,
    load_filters,
    load_pivot_tables,
    load_rows_or_cols,
    load_sparklines,
    load_xlsx_styles,
    read_cell_value,
)

# from openpyxl.utils import coordinate_to_tuple

logger = logging.getLogger("desktopenv.metric.table")

BOOK = Union[pd.ExcelFile, Workbook, str]


def _parse_sheet_idx(
    sheet_idx: Union[int, str],
    result: BOOK,
    expected: BOOK,
    result_sheet_names: List[str],
    expected_sheet_names: List[str],
) -> Tuple[BOOK, str]:
    #  function _parse_sheet_idx {{{ #
    if isinstance(sheet_idx, int):
        try:
            index: str = result_sheet_names[sheet_idx]
        except (IndexError, KeyError):
            index = ""
        book: BOOK = result
    elif sheet_idx.startswith("RI"):
        try:
            index: str = result_sheet_names[int(sheet_idx[2:])]
        except (IndexError, KeyError, ValueError):
            index = ""
        book: BOOK = result
    elif sheet_idx.startswith("RN"):
        index: str = sheet_idx[2:]
        book: BOOK = result
    elif sheet_idx.startswith("EI"):
        try:
            index: str = expected_sheet_names[int(sheet_idx[2:])]
        except (IndexError, KeyError, ValueError):
            index = ""
        book: BOOK = expected
    elif sheet_idx.startswith("EN"):
        index: str = sheet_idx[2:]
        book: BOOK = expected
    else:
        logger.error("Unrecognized sheet index")
        raise ValueError("Unrecognized sheet index")
    return book, index
    #  }}} function _parse_sheet_idx #


SHEET = Union[pd.DataFrame, Worksheet, List[str]]


def _load_sheet(book: BOOK, index: str) -> SHEET:
    #  function _load_sheet {{{ #
    try:
        if isinstance(book, str):
            book: str = cast(str, book)
            csv_name: str = "{:}-{:}.csv".format(os.path.splitext(book)[0], index)

            with open(csv_name) as f:
                csv_lines: List[str] = list(
                    itertools.dropwhile(
                        lambda x: len(x) == 0,
                        map(lambda x: x.strip(), reversed(f.read().splitlines())),
                    )
                )
            return csv_lines
        if isinstance(book, pd.ExcelFile):
            return pd.read_excel(book, index)
        if isinstance(book, Workbook):
            return book[index]
        logger.error("Not supported workbook format")
        raise NotImplementedError("Not supported workbook format")
    except NotImplementedError as e:
        raise e
    except Exception:
        return None
    #  }}} function _load_sheet #


def compare_table(result: str, expected: str = None, **options) -> float:
    #  function compare_table {{{ #
    """
    Args:
        result (str): path to result xlsx
        expected (str): path to golden xlsx
        rules (List[Dict[str, Any]]): list of dict like
          {
            "type": str,
            <str as parameters>: anything
          }
          as sequential rules

    Returns:
        float: the score
    """

    if result is None:
        return 0.0

    try:
        xlworkbookr: Workbook = openpyxl.load_workbook(filename=result)
        pdworkbookr = pd.ExcelFile(result)
    except Exception:
        return 0.0
    worksheetr_names: List[str] = pdworkbookr.sheet_names

    if expected is not None:
        xlworkbooke: Workbook = openpyxl.load_workbook(filename=expected)
        pdworkbooke = pd.ExcelFile(expected)
        worksheete_names: List[str] = pdworkbooke.sheet_names
    else:
        xlworkbooke: Workbook = None
        pdworkbooke = None
        worksheete_names: List[str] = None

    parse_idx: Callable[[Union[str, int], BOOK, BOOK], Tuple[BOOK, str]] = functools.partial(
        _parse_sheet_idx, result_sheet_names=worksheetr_names, expected_sheet_names=worksheete_names
    )

    passes = True
    for r in options["rules"]:
        if r["type"] == "sheet_name":
            #  Compare Sheet Names {{{ #
            metric: bool = worksheetr_names == worksheete_names
            logger.debug(
                "Assertion: %s.sheet_names == %s.sheet_names - %s", result, expected, metric
            )
            #  }}} Compare Sheet Names #

        elif r["type"] == "sheet_data":
            #  Compare Sheet Data by Internal Value {{{ #
            # sheet_idx0: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # sheet_idx1: as sheet_idx0
            # precision: int as number of decimal digits, default to 4

            error_limit: int = r.get("precision", 4)
            sheet1: pd.DataFrame = _load_sheet(
                *parse_idx(r["sheet_idx0"], pdworkbookr, pdworkbooke)
            )
            if sheet1 is None:
                return 0.0
            sheet2: pd.DataFrame = _load_sheet(
                *parse_idx(r["sheet_idx1"], pdworkbookr, pdworkbooke)
            )

            sheet1 = sheet1.round(error_limit)
            sheet2 = sheet2.round(error_limit)
            metric: bool = sheet1.equals(sheet2)
            logger.debug("Sheet1: \n%s", str(sheet1))
            logger.debug("Sheet2: \n%s", str(sheet2))
            try:
                logger.debug("Sheet1 =v= Sheet2: \n%s", str(sheet1 == sheet2))
            except Exception:
                logger.debug("Sheet1 =/v= Sheet2")
            logger.debug("Assertion: %s =v= %s - %s", r["sheet_idx0"], r["sheet_idx1"], metric)
            #  }}} Compare Sheet Data by Internal Value #

        elif r["type"] == "sheet_print":
            #  Compare Sheet Data by Printed Value {{{ #
            # sheet_idx0: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # sheet_idx1: as sheet_idx0
            # ignore_case: optional, defaults to False

            sheet1: List[str] = _load_sheet(*parse_idx(r["sheet_idx0"], result, expected))
            if sheet1 is None:
                return 0.0
            sheet2: List[str] = _load_sheet(*parse_idx(r["sheet_idx1"], result, expected))
            if r.get("ignore_case", False):
                sheet1 = [item.lower() for item in sheet1]
                sheet2 = [item.lower() for item in sheet2]
            metric: bool = sheet1 == sheet2
            logger.debug("Assertion: %s =p= %s - %s", r["sheet_idx0"], r["sheet_idx1"], metric)
            #  }}} Compare Sheet Data by Printed Value #

        elif r["type"] == "sheet_fuzzy":
            #  Fuzzy Match for Ranges {{{ #
            # sheet_idx0: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # sheet_idx1: as sheet_idx0
            # rules: list of dict, each dict is like
            #   { "range": ["A1:B6", "C2:E5"],
            #     "type": "includes" | "included_by" | "fuzzy_match" | "exact_match", # 0 includes 1, 0 includes_by 1
            #     "threshold": 85, // for fuzzy match
            #     "ignore_case": true | false,
            #     "ignore_chars": " ()", # filtered out
            #     "trim_leadings": "+ ", # filtered by lstrip
            #     "trim_trailings": "", # filtered by rstrip
            #     "normalization": [["Rd", "Road"]], # filtered by replace
            #   }

            sheet1: Tuple[BOOK, str] = parse_idx(r["sheet_idx0"], result, expected)
            sheet2: Tuple[BOOK, str] = parse_idx(r["sheet_idx1"], result, expected)
            total_metric = True
            for rl in r["rules"]:
                for rng in MultiCellRange(rl["range"]):
                    for cdn in rng.cells:
                        coordinate: str = "{:}{:d}".format(get_column_letter(cdn[1]), cdn[0])
                        value1: str = str(read_cell_value(*sheet1, coordinate))
                        value2: str = str(read_cell_value(*sheet2, coordinate))
                        logger.debug("%s: %s vs %s", cdn, value1, value2)

                        for rplc in rl.get("normalization", []):
                            value1 = value1.replace(rplc[0], rplc[1])
                            value2 = value2.replace(rplc[0], rplc[1])
                        if "trim_leadings" in rl:
                            value1 = value1.lstrip(rl["trim_leadings"])
                            value2 = value2.lstrip(rl["trim_leadings"])
                        if "trim_trailings" in rl:
                            value1 = value1.rstrip(rl["trim_trailings"])
                            value2 = value2.rstrip(rl["trim_trailings"])
                        if "ignore_chars" in rl:
                            ignore_chars: Set[str] = set(rl["ignore_chars"])
                            value1 = "".join(filter(lambda ch: ch not in ignore_chars, value1))
                            value2 = "".join(filter(lambda ch: ch not in ignore_chars, value2))
                        if rl.get("ignore_case", False):
                            value1 = value1.lower()
                            value2 = value2.lower()

                        if rl["type"] == "includes":
                            metric: bool = value2 in value1
                        elif rl["type"] == "included_by":
                            metric: bool = value1 in value2
                        elif rl["type"] == "fuzzy_match":
                            metric: bool = fuzz.ratio(value1, value2) >= rl.get("threshold", 85.0)
                        elif rl["type"] == "exact_match":
                            metric: bool = value1 == value2
                        total_metric = total_metric and metric

            metric: bool = total_metric
            logger.debug("Assertion: %s =~= %s - %s", r["sheet_idx0"], r["sheet_idx1"], metric)
            #  }}} Fuzzy Match for Ranges #

        elif r["type"] == "sparkline":
            #  Compare Sparklines {{{ #
            # sheet_idx0: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # sheet_idx1: as sheet_idx0

            sparkline1: Dict[str, str] = load_sparklines(
                *parse_idx(r["sheet_idx0"], result, expected)
            )
            sparkline2: Dict[str, str] = load_sparklines(
                *parse_idx(r["sheet_idx1"], result, expected)
            )
            metric: bool = sparkline1 == sparkline2
            logger.debug("Assertion: %s.sp == %.sp - %s", r["sheet_idx0"], r["sheet_idx1"], metric)
            #  }}} Compare Sparklines #

        elif r["type"] == "chart":
            #  Compare Charts {{{ #
            # sheet_idx0: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # sheet_idx1: as sheet_idx0
            # chart_props: list of str, see utils.load_charts

            charts1: Dict[str, Any] = load_charts(
                *parse_idx(r["sheet_idx0"], xlworkbookr, xlworkbooke), **r
            )
            charts2: Dict[str, Any] = load_charts(
                *parse_idx(r["sheet_idx1"], xlworkbookr, xlworkbooke), **r
            )
            metric: bool = charts1 == charts2
            logger.debug(
                "Assertion: %s[chart] == %s[chart] - %s", r["sheet_idx0"], r["sheet_idx1"], metric
            )
            logger.info(f"charts1: {charts1}")
            logger.info(f"charts2: {charts2}")

            logger.info(
                "Assertion: %s[chart] == %s[chart] - %s", r["sheet_idx0"], r["sheet_idx1"], metric
            )
            #  }}} Compare Charts #

        elif r["type"] == "style":
            #  Compare Style (Also Conditional Formatiing) {{{ #
            # sheet_idx0: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # sheet_idx1: as sheet_idx0
            # props: list of str indicating concerned styles, see utils._read_cell_style

            sheet_idx1: Tuple[BOOK, str] = parse_idx(r["sheet_idx0"], xlworkbookr, xlworkbooke)
            book_name1: str = parse_idx(r["sheet_idx0"], result, expected)[0]
            styles1: Dict[str, List[Any]] = load_xlsx_styles(*sheet_idx1, book_name1, **r)

            sheet_idx2: Tuple[BOOK, str] = parse_idx(r["sheet_idx1"], xlworkbookr, xlworkbooke)
            book_name2: str = parse_idx(r["sheet_idx1"], result, expected)[0]
            styles2: Dict[str, List[Any]] = load_xlsx_styles(*sheet_idx2, book_name2, **r)
            # number_formats1: List[str] = [c.number_format.lower() for col in sheet1.iter_cols() for c in col if c.value is not None and c.data_type=="n"]
            # number_formats2: List[str] = [c.number_format.lower() for col in sheet2.iter_cols() for c in col if c.value is not None and c.data_type=="n"]
            metric: bool = styles1 == styles2
            logger.debug(
                "Assertion: %s.style == %s.style - %s", r["sheet_idx0"], r["sheet_idx1"], metric
            )
            #  }}} Compare Style (Also Conditional Formatiing) #

        elif r["type"] == "freeze":
            #  Compare Freezing {{{ #
            # sheet_idx0: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # sheet_idx1: as sheet_idx0

            sheet1: Worksheet = _load_sheet(*parse_idx(r["sheet_idx0"], xlworkbookr, xlworkbooke))
            if sheet1 is None:
                return 0.0
            sheet2: Worksheet = _load_sheet(*parse_idx(r["sheet_idx1"], xlworkbookr, xlworkbooke))
            metric: bool = sheet1.freeze_panes == sheet2.freeze_panes
            logger.debug(
                "Assertion: %s.freeze(%s) == %s.freeze(%s) - %s",
                r["sheet_idx0"],
                sheet1.freeze_panes,
                r["sheet_idx1"],
                sheet2.freeze_panes,
                metric,
            )
            #  }}} Compare Freezing #

        elif r["type"] == "zoom":
            #  Check Zooming {{{ #
            # sheet_idx: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # method: str
            # ref: value

            sheet: Worksheet = _load_sheet(*parse_idx(r["sheet_idx"], xlworkbookr, xlworkbooke))
            if sheet is None:
                return 0.0
            zoom_scale: Number = sheet.sheet_view.zoomScale or 100.0
            metric: bool = _match_value_to_rule(zoom_scale, r)
            logger.debug(
                "Assertion: %s.zoom(%.1f) %s %.1f - %s",
                r["sheet_idx"],
                zoom_scale,
                r["method"],
                r["ref"],
                metric,
            )
            #  }}} Check Zooming #

        elif r["type"] == "data_validation":
            #  Check Data Validation {{{ #
            # sheet_idx: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # dv_props: list of dict like {attribute: {"method": str, "ref": anything}}
            #   available attributes:
            #     * ranges
            #     * type
            #     * formula1
            #     * formula2
            #     * operator
            #     * allowBlank
            #     * showDropDown
            #     * showInputMessage
            #     * showErrorMessage
            #     * error
            #     * errorTitle
            #     * errorStyle
            #     * prompt
            #     * promptTitle
            #     * imeMode

            sheet: Worksheet = _load_sheet(*parse_idx(r["sheet_idx"], xlworkbookr, xlworkbooke))
            if sheet is None:
                return 0.0
            data_validators: List[DataValidation] = sheet.data_validations.dataValidation

            total_metric = len(data_validators) >= len(r["dv_props"])
            for dat_vldt in data_validators:
                metric = False
                for prpt in r["dv_props"]:
                    metric = metric or all(
                        _match_value_to_rule(getattr(dat_vldt, attrbt), mr)
                        for attrbt, mr in prpt.items()
                    )
                    if metric:
                        break
                total_metric = total_metric and metric
                if not total_metric:
                    break

            logger.debug("Assertion: %s.data_validation - %s", r["sheet_idx"], total_metric)
            metric: bool = total_metric
            #  }}} Check Data Validation #

        elif r["type"] == "row_props":
            #  Check Row Properties {{{ #
            # sheet_idx0: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # sheet_idx1: as sheet_idx0
            # props: list of str, see utils.load_rows_or_cols

            rows1: Dict[str, Any] = load_rows_or_cols(
                *parse_idx(r["sheet_idx0"], xlworkbookr, xlworkbooke), obj="row", **r
            )
            rows2: Dict[str, Any] = load_rows_or_cols(
                *parse_idx(r["sheet_idx1"], xlworkbookr, xlworkbooke), obj="row", **r
            )
            logger.debug("Rows1: %s", repr(rows1))
            logger.debug("Rows2: %s", repr(rows2))
            metric: bool = rows1 == rows2
            logger.debug(
                "Assertion: %s[rows] == %s[rows] - %s", r["sheet_idx0"], r["sheet_idx1"], metric
            )
            #  }}} Check Row Properties #

        elif r["type"] == "col_props":
            #  Check Row Properties {{{ #
            # sheet_idx0: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # sheet_idx1: as sheet_idx0
            # props: list of str, see utils.load_rows_or_cols

            cols1: Dict[str, Any] = load_rows_or_cols(
                *parse_idx(r["sheet_idx0"], xlworkbookr, xlworkbooke), obj="column", **r
            )
            cols2: Dict[str, Any] = load_rows_or_cols(
                *parse_idx(r["sheet_idx1"], xlworkbookr, xlworkbooke), obj="column", **r
            )
            metric: bool = cols1 == cols2
            logger.debug(
                "Assertion: %s[cols] == %s[cols] - %s", r["sheet_idx0"], r["sheet_idx1"], metric
            )
            #  }}} Check Row Properties #

        elif r["type"] == "filter":
            #  Compare Filters {{{ #
            # sheet_idx0: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # sheet_idx1: as sheet_idx0

            filters1: Dict[str, Any] = load_filters(
                *parse_idx(r["sheet_idx0"], xlworkbookr, xlworkbooke), **r
            )
            filters2: Dict[str, Any] = load_filters(
                *parse_idx(r["sheet_idx1"], xlworkbookr, xlworkbooke), **r
            )
            metric: bool = filters1 == filters2
            logger.debug(
                "Assertion: %s[filter] == %s[filter] - %s", r["sheet_idx0"], r["sheet_idx1"], metric
            )
            #  }}} Compare Filters #

        elif r["type"] == "pivot_table":
            #  Compare Pivot Tables {{{ #
            # sheet_idx0: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # sheet_idx1: as sheet_idx0
            # pivot_props: list of str, see utils.load_pivot_tables

            pivots1: Dict[str, Any] = load_pivot_tables(
                *parse_idx(r["sheet_idx0"], xlworkbookr, xlworkbooke), **r
            )
            pivots2: Dict[str, Any] = load_pivot_tables(
                *parse_idx(r["sheet_idx1"], xlworkbookr, xlworkbooke), **r
            )
            metric: bool = pivots1 == pivots2
            logger.debug(
                "Assertion: %s[pivot]==%s[pivot] - %s", r["sheet_idx0"], r["sheet_idx1"], metric
            )
            #  }}} Compare Pivot Tables #

        elif r["type"] == "check_cell":
            #  Check Cell Properties {{{ #
            # sheet_idx: 0 == "RI0" == "RNSheet1" | "EI0" == "ENSheet1"
            # coordinate: str, "E3"
            # props: dict like {attribute: {"method": str, "ref": anything}}
            #   supported attributes: value & those supported by utils._read_cell_style

            sheet: Worksheet = _load_sheet(*parse_idx(r["sheet_idx"], xlworkbookr, xlworkbooke))
            if sheet is None:
                return 0.0
            # data_frame: pd.DataFrame = _load_sheet(*parse_idx(r["sheet_idx"], pdworkbookr, pdworkbooke))
            cell: Cell = sheet[r["coordinate"]]
            metric: bool = True
            for prpt, rule in r["props"].items():
                if prpt == "value":
                    val = read_cell_value(
                        *parse_idx(r["sheet_idx"], result, expected), r["coordinate"]
                    )
                else:
                    val = _read_cell_style(prpt, cell)

                metric = metric and _match_value_to_rule(val, rule)

            logger.debug(
                "Assertion: %s[%s] :%s - %s",
                r["sheet_idx"],
                r["coordinate"],
                repr(r["props"]),
                metric,
            )
            #  }}} Check Cell Properties #

        else:
            raise NotImplementedError("Unimplemented sheet check: {:}".format(r["type"]))

        passes = passes and metric
        if not passes:
            break

    return float(passes)
    #  }}} function compare_table #


def compare_csv(result: str, expected: str, **options) -> float:
    if result is None:
        return 0.0

    with open(result) as f:
        result_lines: Iterable[str] = f.read().splitlines()
    with open(expected) as f:
        expected_lines: Iterable[str] = f.read().splitlines()
    if not options.get("strict", True):
        result_lines = map(str.strip, result_lines)
        expected_lines = map(str.strip, expected_lines)
    if options.get("ignore_case", False):
        result_lines = map(str.lower, result_lines)
        expected_lines = map(str.lower, expected_lines)

    metric: bool = list(result_lines) == list(expected_lines)
    return float(metric)


def compare_conference_city_in_order(actual_city_list_path, expected_city):
    expected_city_list = expected_city["expected"]
    wb = openpyxl.load_workbook(actual_city_list_path)
    sheet = wb.active
    actual_city_list = []
    for row in sheet["C2:C22"]:
        for cell in row:
            actual_city_list.append(cell.value)
    # expected_city is the city that we want to compare with the actual city list
    # must in order index
    # debug
    try:
        for i in range(len(actual_city_list)):
            if isinstance(expected_city_list[i], str):
                if expected_city_list[i] not in actual_city_list[i]:
                    logger.debug(
                        f"Expected city {expected_city_list[i]}; Actual city {actual_city_list[i]}"
                    )
                    print(
                        f"Expected city {expected_city_list[i]}; Actual city {actual_city_list[i]}"
                    )
                    return 0.0

            elif isinstance(expected_city_list[i], List):
                if not any(
                    possible_str in actual_city_list[i] for possible_str in expected_city_list[i]
                ):
                    logger.debug(
                        f"Expected city {expected_city_list[i]}; Actual city {actual_city_list[i]}"
                    )
                    print(
                        f"Expected city {expected_city_list[i]}; Actual city {actual_city_list[i]}"
                    )
                    return 0.0

            else:
                raise TypeError("Expected city should be a string or a list of strings")

    except Exception:
        return 0.0

    return 1.0
