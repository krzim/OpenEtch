// English strings of a Ruida gantry's windows: the File List (stored files, Send, Start, Delete), File > Save RD file
// and the New Ruida device form. Spread into EN (en.ts).

export const EN_RUIDA = {
  "win.files": "File List",

  "rd.refresh": "Refresh",
  "rd.empty": "No files are stored on the controller.",
  "rd.name": "Name",
  "rd.start": "Start…",
  "rd.startTitle": "Ask for a run of this stored file: it waits in the Laser window's Runs and starts only when you approve it there",
  "rd.startRequested": "Run of \"{name}\" requested: approve it in the Laser window's Runs to start it.",
  "rd.delete": "Delete",
  "rd.confirmDelete": "Delete it",
  "rd.deleted": "Deleted \"{name}\" from the controller.",
  "rd.send": "Send the current job",
  "rd.sendButton": "Send",
  "rd.sendTitle": "Store the job on the controller under this name (it is not started)",
  "rd.onlySelected": "Only the selected graphics",
  "rd.sent": "Stored \"{name}\" on the controller.",
  "rd.hint": "Send stores a job in the controller's memory without starting it; start it here (after approval in the Laser window) or from the controller's panel. Ruida support is unverified on hardware.",

  "fm.saveRd": "Save RD file…",
  "fm.rdSaved": "Saved {name}.",

  "dv.newRuida": "New Ruida…",
  "dv.newRuidaHint": "Add a Ruida DSP gantry laser on the network (UDP; unverified on hardware)",
  "dv.ruidaAddress": "Controller IP address",
  "dv.ruidaMaxPower": "Max power (%)",
  "dv.ruidaMaxSpeed": "Max speed (mm/s)",
  "dv.ruida634xg": "RDC634XG controller",
  "dv.ruidaAdded": "Added {name}. Make it active to connect.",
} as const;
