"use strict";
(function () { var form = document.getElementById("inspect-form"); if (!form) { return; } form.addEventListener("submit", function () { var button = form.querySelector("button"); button.disabled = true; button.textContent = "Inspecting…"; }); })();
